#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

if [ "${1:-}" = "run" ] && [ "$(uname -s)" = "Darwin" ]; then
    shift
    run_profile="debug"
    binary_dir="debug"

    while [ "$#" -gt 0 ]; do
        case "$1" in
            --release)
                run_profile="release"
                binary_dir="release"
                shift
                ;;
            --profile)
                if [ "$#" -lt 2 ]; then
                    echo "error: --profile requires dev or release" >&2
                    exit 2
                fi
                case "$2" in
                    dev)
                        run_profile="debug"
                        binary_dir="debug"
                        ;;
                    release)
                        run_profile="release"
                        binary_dir="release"
                        ;;
                    *)
                        echo "error: the macOS runner supports only dev and release profiles" >&2
                        exit 2
                        ;;
                esac
                shift 2
                ;;
            --)
                shift
                break
                ;;
            *)
                echo "error: unsupported macOS run option: $1" >&2
                exit 2
                ;;
        esac
    done

    "$project_dir/scripts/bundle-macos.sh" "$run_profile"
    bundle_path="$project_dir/target/$binary_dir/Resolved.app"
    if [ "$#" -gt 0 ]; then
        exec /usr/bin/open -W "$bundle_path" --args "$@"
    fi
    exec /usr/bin/open -W "$bundle_path"
fi

"$project_dir/scripts/prepare-gpui.sh"
"$project_dir/scripts/prepare-typescript-service.sh"
cd "$project_dir"
exec cargo "$@"
