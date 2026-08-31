Resolved portable Linux archive
================================

The archive is relocatable. Extract its top-level directory anywhere and run:

    ./resolved

For a conventional system-wide installation, move that directory to /opt and
make the launcher available on PATH:

    sudo mv Resolved-<version>-linux-<architecture> /opt/resolved
    sudo ln -s /opt/resolved/resolved /usr/local/bin/resolved

The archive carries Resolved's distributable shared-library dependencies,
including GTK 3 and WebKitGTK 4.1, plus WebKitGTK's helper processes. It still
uses the host kernel, glibc/ELF loader, graphics drivers, display server, and
other low-level system interfaces. Build the archive on the oldest supported
Linux distribution to keep its glibc requirement broadly compatible.

Desktop integration resources are under usr/share/applications,
usr/share/icons, and usr/share/metainfo. Copy or symlink them into
/usr/local/share if desired.
