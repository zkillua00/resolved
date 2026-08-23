#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
lockfile="$project_dir/Cargo.lock"

if [ ! -f "$lockfile" ]; then
    echo "error: Cargo.lock is required for a reproducible RustSec audit" >&2
    exit 2
fi

if ! cargo audit --version >/dev/null 2>&1; then
    cat >&2 <<'EOF'
error: cargo-audit is required to check dependencies against RustSec

Install it with one of:
  cargo install --locked cargo-audit
  brew install cargo-audit
EOF
    exit 2
fi

echo "Auditing Cargo.lock against the current RustSec Advisory Database..."
exec cargo audit --file "$lockfile" "$@"
