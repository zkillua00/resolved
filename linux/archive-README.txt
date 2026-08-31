Resolved portable Linux archive
================================

The archive is relocatable. Extract its top-level directory anywhere and run:

    ./resolved

For a conventional system-wide installation, move that directory to /opt and
make the launcher available on PATH:

    sudo mv Resolved-<version>-linux-<architecture> /opt/resolved
    sudo ln -s /opt/resolved/resolved /usr/local/bin/resolved

The host must provide the same Linux runtime libraries as the .deb and .rpm
packages, including GTK 3, WebKitGTK 4.1, Fontconfig, Vulkan, X11, XKB, and XCB.
Desktop integration resources are under share/applications, share/icons, and
share/metainfo. Copy or symlink them into /usr/local/share if desired.
