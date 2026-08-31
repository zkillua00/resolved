#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
profile="${1:-release}"
format="${2:-all}"

case "$profile" in
    debug)
        cargo_arguments="build"
        binary_dir="debug"
        ;;
    release)
        cargo_arguments="build --release"
        binary_dir="release"
        ;;
    *)
        echo "usage: scripts/package-linux.sh [debug|release] [all|deb|rpm|appimage|archive]" >&2
        exit 2
        ;;
esac

case "$format" in
    all | deb | rpm | appimage | archive) ;;
    *)
        echo "usage: scripts/package-linux.sh [debug|release] [all|deb|rpm|appimage|archive]" >&2
        exit 2
        ;;
esac

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: Linux packages must be built on Linux" >&2
    exit 2
fi

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: $1 is required to create the requested Linux package" >&2
        exit 2
    fi
}

wants_format() {
    [ "$format" = "all" ] || [ "$format" = "$1" ]
}

require_command install
require_command tar
if wants_format deb; then
    require_command dpkg-deb
fi
if wants_format rpm; then
    require_command rpmbuild
fi
if wants_format appimage; then
    require_command appimagetool
fi

# Intentional word splitting: this is the fixed build command selected above.
# shellcheck disable=SC2086
"$project_dir/scripts/cargo.sh" $cargo_arguments

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$project_dir/Cargo.toml" | sed -n '1p')"
case "$(uname -m)" in
    x86_64)
        deb_architecture="amd64"
        rpm_architecture="x86_64"
        portable_architecture="x86_64"
        ;;
    aarch64 | arm64)
        deb_architecture="arm64"
        rpm_architecture="aarch64"
        portable_architecture="aarch64"
        ;;
    *)
        echo "error: unsupported Linux package architecture: $(uname -m)" >&2
        exit 2
        ;;
esac

staging_dir="$(mktemp -d "${TMPDIR:-/tmp}/resolved-linux.XXXXXX")"
trap 'rm -rf "$staging_dir"' EXIT HUP INT TERM
payload_root="$staging_dir/payload"

install_payload() {
    root="$1"
    install -d \
        "$root/usr/bin" \
        "$root/usr/share/applications" \
        "$root/usr/share/icons/hicolor/1024x1024/apps" \
        "$root/usr/share/metainfo"
    install -m 755 "$project_dir/target/$binary_dir/api-tester" "$root/usr/bin/resolved"
    install -m 644 \
        "$project_dir/linux/io.github.zkillua00.resolved.desktop" \
        "$root/usr/share/applications/io.github.zkillua00.resolved.desktop"
    install -m 644 \
        "$project_dir/linux/io.github.zkillua00.resolved.appdata.xml" \
        "$root/usr/share/metainfo/io.github.zkillua00.resolved.appdata.xml"
    install -m 644 \
        "$project_dir/assets/brand/resolved-icon.png" \
        "$root/usr/share/icons/hicolor/1024x1024/apps/io.github.zkillua00.resolved.png"
}

install_payload "$payload_root"

if wants_format deb; then
    deb_root="$staging_dir/deb"
    install -d "$deb_root/DEBIAN"
    cp -a "$payload_root/usr" "$deb_root/"
    cat >"$deb_root/DEBIAN/control" <<EOF
Package: resolved
Version: $version
Section: devel
Priority: optional
Architecture: $deb_architecture
Maintainer: Resolved contributors
Depends: libc6, libfontconfig1, libgcc-s1, libgtk-3-0, libvulkan1, libwebkit2gtk-4.1-0, libx11-6, libxcb1, libxkbcommon0, libxkbcommon-x11-0
Description: Native API workbench
 Know exactly what you sent. Resolved keeps request intent, wire state,
 and responses explicit.
EOF

    deb_output="$project_dir/target/$binary_dir/Resolved-$version-linux-$deb_architecture.deb"
    dpkg-deb --build --root-owner-group "$deb_root" "$deb_output"
    echo "Created $deb_output"
fi

if wants_format rpm; then
    rpm_top="$staging_dir/rpmbuild"
    rpm_spec="$staging_dir/resolved.spec"
    install -d "$rpm_top/BUILD" "$rpm_top/BUILDROOT" "$rpm_top/RPMS" "$rpm_top/SOURCES" "$rpm_top/SPECS" "$rpm_top/SRPMS"
    cat >"$rpm_spec" <<EOF
Name: resolved
Version: $version
Release: 1
Summary: Native API workbench
License: Apache-2.0
URL: https://github.com/zkillua00/resolved
BuildArch: $rpm_architecture
AutoReqProv: yes

%description
Know exactly what you sent. Resolved keeps request intent, wire state, and
responses explicit.

%prep

%build

%install
mkdir -p %{buildroot}
cp -a "$payload_root/usr" %{buildroot}/

%files
%{_bindir}/resolved
%{_datadir}/applications/io.github.zkillua00.resolved.desktop
%{_datadir}/icons/hicolor/1024x1024/apps/io.github.zkillua00.resolved.png
%{_datadir}/metainfo/io.github.zkillua00.resolved.appdata.xml
EOF

    rpmbuild \
        --define "_topdir $rpm_top" \
        --define "_build_id_links none" \
        --target "$rpm_architecture" \
        -bb "$rpm_spec"
    built_rpm="$rpm_top/RPMS/$rpm_architecture/resolved-$version-1.$rpm_architecture.rpm"
    rpm_output="$project_dir/target/$binary_dir/Resolved-$version-linux-$rpm_architecture.rpm"
    install -m 644 "$built_rpm" "$rpm_output"
    echo "Created $rpm_output"
fi

if wants_format archive; then
    archive_name="Resolved-$version-linux-$portable_architecture"
    archive_root="$staging_dir/$archive_name"
    install -d "$archive_root/bin" "$archive_root/share"
    install -m 755 "$payload_root/usr/bin/resolved" "$archive_root/bin/resolved"
    cp -a "$payload_root/usr/share/." "$archive_root/share/"
    install -m 755 "$project_dir/linux/archive-launcher.sh" "$archive_root/resolved"
    install -m 644 "$project_dir/linux/archive-README.txt" "$archive_root/README.txt"

    archive_output="$project_dir/target/$binary_dir/$archive_name.tar.xz"
    tar \
        --sort=name \
        --owner=0 \
        --group=0 \
        --numeric-owner \
        --mtime="@${SOURCE_DATE_EPOCH:-0}" \
        -C "$staging_dir" \
        -cJf "$archive_output" \
        "$archive_name"
    echo "Created $archive_output"
fi

if wants_format appimage; then
    appdir="$staging_dir/Resolved.AppDir"
    appimage_output="$project_dir/target/$binary_dir/Resolved-$version-linux-$portable_architecture.AppImage"
    install -d \
        "$appdir/usr/bin" \
        "$appdir/usr/share/applications" \
        "$appdir/usr/share/icons/hicolor/256x256/apps" \
        "$appdir/usr/share/metainfo"
    install -m 755 "$payload_root/usr/bin/resolved" "$appdir/usr/bin/resolved"
    install -m 644 \
        "$project_dir/linux/io.github.zkillua00.resolved.desktop" \
        "$appdir/usr/share/applications/io.github.zkillua00.resolved.desktop"
    install -m 644 \
        "$project_dir/linux/io.github.zkillua00.resolved.appdata.xml" \
        "$appdir/usr/share/metainfo/io.github.zkillua00.resolved.appdata.xml"
    install -m 644 \
        "$project_dir/assets/brand/resolved-runtime.png" \
        "$appdir/usr/share/icons/hicolor/256x256/apps/io.github.zkillua00.resolved.png"
    ln -s usr/bin/resolved "$appdir/AppRun"
    ln -s \
        usr/share/applications/io.github.zkillua00.resolved.desktop \
        "$appdir/io.github.zkillua00.resolved.desktop"
    ln -s \
        usr/share/icons/hicolor/256x256/apps/io.github.zkillua00.resolved.png \
        "$appdir/io.github.zkillua00.resolved.png"

    ARCH="$portable_architecture" VERSION="$version" \
        appimagetool \
        --runtime-file /usr/local/lib/appimage/runtime \
        "$appdir" \
        "$appimage_output"
    chmod 755 "$appimage_output"
    echo "Created $appimage_output"
fi
