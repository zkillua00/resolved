#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

"$project_dir/scripts/prepare-gpui.sh"
cd "$project_dir"
exec cargo "$@"
