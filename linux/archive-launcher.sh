#!/bin/sh
set -eu

resolved_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
archive_library_dir="$resolved_dir/usr/lib"

if [ -n "${LD_LIBRARY_PATH:-}" ]; then
    LD_LIBRARY_PATH="$archive_library_dir:$LD_LIBRARY_PATH"
else
    LD_LIBRARY_PATH="$archive_library_dir"
fi
export LD_LIBRARY_PATH

if [ -n "${XDG_DATA_DIRS:-}" ]; then
    XDG_DATA_DIRS="$resolved_dir/usr/share:$XDG_DATA_DIRS"
else
    XDG_DATA_DIRS="$resolved_dir/usr/share:/usr/local/share:/usr/share"
fi
export XDG_DATA_DIRS

WEBKIT_EXEC_PATH="$resolved_dir/usr/libexec/webkit2gtk-4.1"
WEBKIT_INJECTED_BUNDLE_PATH="$resolved_dir/usr/lib/webkit2gtk-4.1/injected-bundle"
export WEBKIT_EXEC_PATH WEBKIT_INJECTED_BUNDLE_PATH

exec "$resolved_dir/usr/bin/resolved" "$@"
