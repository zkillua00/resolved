#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
profile="${1:-release}"
if [ "$#" -gt 0 ]; then shift; fi
if [ "$#" -eq 0 ]; then set -- all; fi
formats="$*"

case "$profile" in
    debug)
        cargo_arguments="build --locked"
        binary_dir="debug"
        ;;
    release)
        cargo_arguments="build --release --locked"
        binary_dir="release"
        ;;
    *)
        echo "usage: scripts/package-linux.sh [debug|release] [all|deb|rpm|appimage|archive]..." >&2
        exit 2
        ;;
esac

for format in "$@"; do
    case "$format" in
        all | deb | rpm | appimage | archive) ;;
        *)
            echo "usage: scripts/package-linux.sh [debug|release] [all|deb|rpm|appimage|archive]..." >&2
            exit 2
            ;;
    esac
done

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
    case " $formats " in
        *" all "* | *" $1 "*) return 0 ;;
        *) return 1 ;;
    esac
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
if wants_format archive; then
    require_command linuxdeploy
    require_command pkg-config
fi

# Intentional word splitting: this is the fixed build command selected above.
# shellcheck disable=SC2086
"$project_dir/scripts/cargo.sh" $cargo_arguments

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$project_dir/Cargo.toml" | sed -n '1p')"
version="${RESOLVED_BUILD_VERSION:-$version}"
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
 Resolved keeps request intent, wire state, and responses explicit.
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
Resolved keeps request intent, wire state, and
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
    install_payload "$archive_root"

    # WebKitGTK runs web content, networking, and GPU work in separate
    # executables. They are not in the main binary's ELF dependency graph, so
    # stage them explicitly before linuxdeploy collects their shared libraries.
    webkit_lib_dir="$(pkg-config --variable=libdir webkit2gtk-4.1)"
    webkit_prefix="$(pkg-config --variable=prefix webkit2gtk-4.1)"
    webkit_helper_dir="$webkit_lib_dir/webkit2gtk-4.1"
    if [ ! -d "$webkit_helper_dir" ]; then
        webkit_helper_dir="$webkit_prefix/libexec/webkit2gtk-4.1"
    fi
    if [ ! -d "$webkit_helper_dir" ]; then
        echo "error: WebKitGTK helper directory not found under $webkit_lib_dir or $webkit_prefix/libexec" >&2
        exit 2
    fi

    archive_webkit_exec_dir="$archive_root/usr/libexec/webkit2gtk-4.1"
    archive_webkit_bundle_dir="$archive_root/usr/lib/webkit2gtk-4.1/injected-bundle"
    install -d "$archive_webkit_exec_dir" "$archive_webkit_bundle_dir"

    found_webkit_helper=false
    for webkit_helper in "$webkit_helper_dir"/WebKit*Process; do
        if [ ! -f "$webkit_helper" ]; then
            continue
        fi
        found_webkit_helper=true
        archived_helper="$archive_webkit_exec_dir/$(basename "$webkit_helper")"
        install -m 755 "$webkit_helper" "$archived_helper"
    done
    if [ "$found_webkit_helper" = false ]; then
        echo "error: no WebKitGTK helper processes found in $webkit_helper_dir" >&2
        exit 2
    fi

    found_injected_bundle=false
    for injected_bundle in "$webkit_lib_dir"/webkit2gtk-4.1/injected-bundle/*.so; do
        if [ ! -f "$injected_bundle" ]; then
            continue
        fi
        found_injected_bundle=true
        archived_bundle="$archive_webkit_bundle_dir/$(basename "$injected_bundle")"
        install -m 755 "$injected_bundle" "$archived_bundle"
    done
    if [ "$found_injected_bundle" = false ]; then
        echo "error: WebKitGTK injected bundle not found under $webkit_lib_dir" >&2
        exit 2
    fi

    # linuxdeploy intentionally leaves glibc, the ELF loader, and low-level
    # host/driver interfaces alone, but copies the distributable dependency
    # closure into usr/lib and gives the copied ELF files relocatable RPATHs.
    APPIMAGE_EXTRACT_AND_RUN=1 linuxdeploy --appdir "$archive_root"
    rm -f "$archive_root/AppRun"

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
