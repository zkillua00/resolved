#!/bin/sh
set -eu

resolved_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
exec "$resolved_dir/bin/resolved" "$@"
