FROM rust:1-bookworm

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        dpkg-dev \
        file \
        git \
        appstream \
        libasound2-dev \
        libfontconfig1-dev \
        libgtk-3-dev \
        libssl-dev \
        libvulkan-dev \
        libwebkit2gtk-4.1-dev \
        libx11-xcb-dev \
        libxcb1-dev \
        libxcb-render0-dev \
        libxcb-shape0-dev \
        libxcb-xfixes0-dev \
        libxkbcommon-dev \
        libxkbcommon-x11-dev \
        patch \
        patchelf \
        pkg-config \
        rpm \
        mesa-vulkan-drivers \
        xauth \
        xvfb \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt

ARG TARGETARCH
ARG LINUXDEPLOY_VERSION=1-alpha-20251107-1
RUN set -eu; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
        amd64) \
            tool_arch=x86_64; \
            tool_sha256=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d \
            ;; \
        arm64) \
            tool_arch=aarch64; \
            tool_sha256=620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff \
            ;; \
        *) \
            echo "unsupported container architecture: ${TARGETARCH:-unknown}" >&2; \
            exit 2 \
            ;; \
    esac; \
    curl --fail --location --silent --show-error \
        --output /usr/local/bin/linuxdeploy \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/${LINUXDEPLOY_VERSION}/linuxdeploy-${tool_arch}.AppImage"; \
    echo "$tool_sha256  /usr/local/bin/linuxdeploy" | sha256sum --check; \
    chmod 755 /usr/local/bin/linuxdeploy; \
    mkdir -p /tmp/linuxdeploy-extract /usr/local/lib/appimage; \
    cd /tmp/linuxdeploy-extract; \
    /usr/local/bin/linuxdeploy --appimage-extract >/dev/null; \
    install -m 755 \
        squashfs-root/plugins/linuxdeploy-plugin-appimage/usr/bin/appimagetool \
        /usr/local/bin/appimagetool; \
    cp -a \
        squashfs-root/plugins/linuxdeploy-plugin-appimage/appimagetool-prefix \
        /usr/appimagetool-prefix; \
    case "$tool_arch" in \
        x86_64) runtime_sha256=1cc49bcf1e2ccd593c379adb17c9f85a36d619088296504de95b1d06215aebbf ;; \
        aarch64) runtime_sha256=7d5d772b7c32f0c84caf0a452a3072a5709027d7eac5856feb89a7a7a8881372 ;; \
    esac; \
    curl --fail --location --silent --show-error \
        --output /usr/local/lib/appimage/runtime \
        "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-${tool_arch}"; \
    echo "$runtime_sha256  /usr/local/lib/appimage/runtime" | sha256sum --check; \
    chmod 755 /usr/local/lib/appimage/runtime; \
    rm -rf /tmp/linuxdeploy-extract; \
    rm /usr/local/bin/linuxdeploy

WORKDIR /workspace
