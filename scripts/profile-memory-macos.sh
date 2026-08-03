#!/bin/sh
set -eu

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
bundle="$project_dir/target/release/Resolved.app"
duration=30
interval=5
settled_samples=3
warmup=0
focus_settle=3
output_dir=""
background=0
profile_home=""

usage() {
    usage_status="${1:-2}"
    cat >&2 <<EOF
usage: $0 [--bundle PATH] [--duration SECONDS] [--interval SECONDS]
          [--settled-samples COUNT] [--warmup SECONDS]
          [--focus-settle SECONDS] [--background] [--home DIR]
          [--output DIR]

Launches one fresh Resolved process, captures macOS physical-footprint and heap
diagnostics, and stops only that process. By default the app stays frontmost;
--background launches it hidden and rejects the run if it becomes frontmost.
EOF
    exit "$usage_status"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --bundle)
            [ "$#" -ge 2 ] || usage
            bundle="$2"
            shift 2
            ;;
        --duration)
            [ "$#" -ge 2 ] || usage
            duration="$2"
            shift 2
            ;;
        --interval)
            [ "$#" -ge 2 ] || usage
            interval="$2"
            shift 2
            ;;
        --settled-samples)
            [ "$#" -ge 2 ] || usage
            settled_samples="$2"
            shift 2
            ;;
        --warmup)
            [ "$#" -ge 2 ] || usage
            warmup="$2"
            shift 2
            ;;
        --focus-settle)
            [ "$#" -ge 2 ] || usage
            focus_settle="$2"
            shift 2
            ;;
        --background)
            background=1
            shift
            ;;
        --home)
            [ "$#" -ge 2 ] || usage
            profile_home="$2"
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || usage
            output_dir="$2"
            shift 2
            ;;
        -h|--help)
            usage 0
            ;;
        *)
            usage
            ;;
    esac
done

case "$duration" in
    ''|*[!0-9]*) usage ;;
esac
case "$interval" in
    ''|*[!0-9]*) usage ;;
esac
case "$settled_samples" in
    ''|*[!0-9]*) usage ;;
esac
case "$warmup" in
    ''|*[!0-9]*) usage ;;
esac
case "$focus_settle" in
    ''|*[!0-9]*) usage ;;
esac
if [ "$duration" -lt 5 ] || [ "$interval" -lt 1 ] ||
    [ "$interval" -gt "$duration" ] || [ "$settled_samples" -lt 1 ] ||
    [ "$focus_settle" -lt 1 ]; then
    usage
fi
if [ -n "$profile_home" ] && [ ! -d "$profile_home" ]; then
    echo "error: profile HOME does not exist: $profile_home" >&2
    exit 1
fi
if [ -n "$profile_home" ]; then
    profile_home="$(CDPATH= cd -- "$profile_home" && pwd)"
fi

bundle="$(CDPATH= cd -- "$(dirname -- "$bundle")" && pwd)/$(basename -- "$bundle")"
executable="$bundle/Contents/MacOS/api-tester"
if [ ! -x "$executable" ]; then
    echo "error: missing executable at $executable" >&2
    exit 1
fi

if [ -z "$output_dir" ]; then
    output_dir="$project_dir/target/memory-profiles/$(date '+%Y%m%d-%H%M%S')"
fi
if [ -e "$output_dir" ]; then
    echo "error: output path already exists: $output_dir" >&2
    exit 1
fi
mkdir -p "$output_dir"
output_dir="$(CDPATH= cd -- "$output_dir" && pwd)"

profile_home_manifest_sha256=""
profile_home_regular_file_count=0
profile_home_regular_file_bytes=0
if [ -n "$profile_home" ]; then
    profile_home_manifest="$output_dir/profile-home-manifest.tsv"
    (
        CDPATH= cd -- "$profile_home"
        find . -type f -print | LC_ALL=C sort |
            while IFS= read -r relative_path; do
                file_sha256="$(shasum -a 256 "$relative_path" | awk '{print $1}')"
                file_bytes="$(wc -c <"$relative_path" | tr -d '[:space:]')"
                printf '%s\t%s\t%s\n' "$file_sha256" "$file_bytes" "$relative_path"
            done
    ) >"$profile_home_manifest"
    profile_home_manifest_sha256="$(shasum -a 256 "$profile_home_manifest" | awk '{print $1}')"
    profile_home_regular_file_count="$(wc -l <"$profile_home_manifest" | tr -d '[:space:]')"
    profile_home_regular_file_bytes="$(awk -F '\t' '{ total += $2 } END { printf "%.0f", total }' "$profile_home_manifest")"
fi

find_pid() {
    ps -ww -axo pid=,command= | awk -v executable="$executable" '
        {
            pid = $1
            command_line = $0
            sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", command_line)
            if (command_line == executable) {
                print pid
                exit
            }
        }
    '
}

find_any_api_tester_pid() {
    pgrep -x api-tester 2>/dev/null | sed -n '1p'
}

front_pid() {
    front_asn="$(lsappinfo front 2>/dev/null || true)"
    [ -n "$front_asn" ] || return 0
    lsappinfo info -only pid "$front_asn" 2>/dev/null |
        sed -n 's/.*"pid"=\([0-9][0-9]*\).*/\1/p'
}

pid="$(find_any_api_tester_pid)"
if [ -n "$pid" ]; then
    echo "error: another api-tester process is already running as PID $pid" >&2
    exit 1
fi

monitor_pid=""
process_matches_executable() {
    [ -n "$pid" ] || return 1
    [ "$(ps -ww -p "$pid" -o command= 2>/dev/null || true)" = "$executable" ]
}

stop_app() {
    if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
        pid=""
        return 0
    fi
    if ! process_matches_executable; then
        echo "error: PID $pid no longer belongs to $executable; refusing to terminate it" >&2
        return 1
    fi

    kill "$pid"
    stop_attempt=0
    while [ "$stop_attempt" -lt 40 ] && kill -0 "$pid" 2>/dev/null; do
        stop_attempt=$((stop_attempt + 1))
        sleep 0.25
    done
    if kill -0 "$pid" 2>/dev/null; then
        echo "error: Resolved PID $pid did not terminate after SIGTERM" >&2
        return 1
    fi
    pid=""
}

cleanup() {
    if [ -n "$monitor_pid" ]; then
        kill "$monitor_pid" 2>/dev/null || true
        wait "$monitor_pid" 2>/dev/null || true
        monitor_pid=""
    fi
    stop_app 2>/dev/null || true
}
trap cleanup EXIT HUP INT TERM

if [ "$background" -eq 1 ]; then
    if [ -n "$profile_home" ]; then
        open -gj -n --env "HOME=$profile_home" "$bundle"
    else
        open -gj -n "$bundle"
    fi
elif [ -n "$profile_home" ]; then
    open -n --env "HOME=$profile_home" "$bundle"
else
    open -n "$bundle"
fi
attempt=0
while [ "$attempt" -lt 40 ]; do
    pid="$(find_pid)"
    [ -n "$pid" ] && break
    attempt=$((attempt + 1))
    sleep 0.25
done
if [ -z "$pid" ]; then
    echo "error: Resolved did not launch" >&2
    exit 1
fi

if [ "$background" -eq 0 ]; then
    open "$bundle"
    attempt=0
    stable_front_samples=0
    # Include both endpoints so the first-to-last successful polls span the full
    # requested interval (four 250 ms gaps per second).
    stable_front_required=$((focus_settle * 4 + 1))
    activation_attempt_limit=$((stable_front_required + 240))
    while [ "$attempt" -lt "$activation_attempt_limit" ]; do
        if [ "$(front_pid)" = "$pid" ]; then
            stable_front_samples=$((stable_front_samples + 1))
            if [ "$stable_front_samples" -ge "$stable_front_required" ]; then
                break
            fi
        else
            stable_front_samples=0
            open "$bundle"
        fi
        attempt=$((attempt + 1))
        sleep 0.25
    done
    if [ "$stable_front_samples" -lt "$stable_front_required" ] || [ "$(front_pid)" != "$pid" ]; then
        echo "error: Resolved did not remain frontmost for $focus_settle seconds" >&2
        exit 1
    fi
fi

focus_log="$output_dir/focus.tsv"
focus_invalid="$output_dir/focus-invalid"
focus_monitor_failed="$output_dir/focus-monitor-failed"
printf 'timestamp\texpected_pid\tfront_pid\n' >"$focus_log"
(
    while kill -0 "$pid" 2>/dev/null; do
        observed_pid="$(front_pid)"
        printf '%s\t%s\t%s\n' "$(date '+%s')" "$pid" "${observed_pid:-none}" >>"$focus_log"
        if [ "$background" -eq 0 ] && [ "$observed_pid" != "$pid" ]; then
            : >"$focus_invalid"
        fi
        if [ "$background" -eq 1 ] && [ "$observed_pid" = "$pid" ]; then
            : >"$focus_invalid"
        fi
        sleep 0.25
    done
) &
monitor_pid=$!

{
    echo "timestamp=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo "bundle=$bundle"
    echo "pid=$pid"
    echo "duration_seconds=$duration"
    echo "interval_seconds=$interval"
    echo "settled_samples=$settled_samples"
    echo "warmup_seconds=$warmup"
    echo "focus_settle_seconds=$focus_settle"
    if [ "$background" -eq 1 ]; then
        echo "launch_mode=background-hidden"
    else
        echo "launch_mode=foreground-active"
    fi
    if [ -n "$profile_home" ]; then
        echo "profile_home=$profile_home"
        echo "profile_home_manifest_sha256=$profile_home_manifest_sha256"
        echo "profile_home_regular_file_count=$profile_home_regular_file_count"
        echo "profile_home_regular_file_bytes=$profile_home_regular_file_bytes"
    fi
    echo "profiler_checkout_git_sha=$(git -C "$project_dir" rev-parse HEAD 2>/dev/null || true)"
    echo "binary_sha256=$(shasum -a 256 "$executable" | awk '{print $1}')"
    sw_vers
    uname -m
} >"$output_dir/metadata.txt"
git -C "$project_dir" status --short >"$output_dir/profiler-checkout-git-status.txt" 2>/dev/null || true

if [ "$warmup" -gt 0 ]; then
    sleep "$warmup"
fi

footprint --sample "$interval" --sample-duration "$duration" --noCategories -f bytes "$pid" \
    >"$output_dir/footprint-trace.txt"
footprint -f bytes "$pid" >"$output_dir/footprint-categories.txt"
vmmap -summary "$pid" >"$output_dir/vmmap-summary.txt"
vmmap -w "$pid" >"$output_dir/vmmap-full.txt"
/usr/bin/heap -q -s "$pid" >"$output_dir/heap-summary.txt"
ps -ww -p "$pid" -o pid=,rss=,vsz=,etime=,command= >"$output_dir/ps.txt"

if ! kill -0 "$monitor_pid" 2>/dev/null; then
    : >"$focus_monitor_failed"
fi
kill "$monitor_pid" 2>/dev/null || true
wait "$monitor_pid" 2>/dev/null || true
monitor_pid=""

if [ -f "$focus_invalid" ]; then
    if [ "$background" -eq 1 ]; then
        echo "error: Resolved became frontmost during a background capture; discarding run at $output_dir" >&2
    else
        echo "error: frontmost app changed during capture; discarding run at $output_dir" >&2
    fi
    exit 1
fi
if [ -f "$focus_monitor_failed" ]; then
    echo "error: focus monitor stopped unexpectedly; discarding run at $output_dir" >&2
    exit 1
fi

awk -v expected_pid="$pid" '
    NR > 1 {
        polls++
        if ($3 == expected_pid) {
            target_frontmost_polls++
        } else {
            other_front_pids[$3] = 1
        }
    }
    END {
        for (front_pid in other_front_pids) {
            distinct_other_front_pids++
        }
        printf "focus_poll_count=%d\n", polls
        printf "target_frontmost_poll_count=%d\n", target_frontmost_polls
        printf "distinct_other_front_pid_count=%d\n", distinct_other_front_pids
    }
' "$focus_log" >"$output_dir/focus-summary.txt"

if ! awk -v tail_count="$settled_samples" '
    /^[[:space:]]*phys_footprint:/ {
        if ($2 !~ /^[0-9]+$/) {
            invalid = 1
        }
        samples[++sample_count] = $2 + 0
    }
    /^[[:space:]]*phys_footprint_peak:/ {
        if ($2 !~ /^[0-9]+$/) {
            invalid = 1
        }
        if (!peak_seen || $2 + 0 > peak) {
            peak = $2 + 0
        }
        peak_seen = 1
    }
    END {
        if (invalid || sample_count < tail_count || !peak_seen) {
            exit 2
        }

        first = sample_count - tail_count + 1
        for (i = 1; i <= tail_count; i++) {
            sorted[i] = samples[first + i - 1]
        }
        for (i = 2; i <= tail_count; i++) {
            value = sorted[i]
            j = i - 1
            while (j >= 1 && sorted[j] > value) {
                sorted[j + 1] = sorted[j]
                j--
            }
            sorted[j + 1] = value
        }

        if (tail_count % 2 == 1) {
            median = sorted[(tail_count + 1) / 2]
        } else {
            median = (sorted[tail_count / 2] + sorted[tail_count / 2 + 1]) / 2
        }

        printf "sample_count=%d\n", sample_count
        printf "settled_sample_count=%d\n", tail_count
        printf "settled_phys_footprint_median_bytes=%.0f\n", median
        printf "settled_phys_footprint_min_bytes=%.0f\n", sorted[1]
        printf "settled_phys_footprint_max_bytes=%.0f\n", sorted[tail_count]
        printf "process_peak_phys_footprint_bytes=%.0f\n", peak
    }
' "$output_dir/footprint-trace.txt" >"$output_dir/summary.txt"; then
    echo "error: footprint trace did not contain enough valid phys_footprint samples" >&2
    exit 1
fi

if ! awk '
    /^All zones: [0-9]+ nodes \([0-9]+ bytes\)$/ {
        nodes = $3
        bytes = $5
        gsub(/[()]/, "", bytes)
    }
    $4 == "non-object" && untyped_bytes == "" {
        untyped_nodes = $1
        untyped_bytes = $2
    }
    END {
        if (nodes == "" || bytes == "" || untyped_bytes == "") {
            exit 2
        }
        printf "heap_live_nodes=%s\n", nodes
        printf "heap_live_bytes=%s\n", bytes
        printf "heap_untyped_nodes=%s\n", untyped_nodes
        printf "heap_untyped_bytes=%s\n", untyped_bytes
    }
' "$output_dir/heap-summary.txt" >>"$output_dir/summary.txt"; then
    echo "error: heap summary did not contain exact live-allocation totals" >&2
    exit 1
fi

if ! awk '
    $2 == "B" && $4 == "B" && $6 == "B" {
        category = $8
        for (i = 9; i <= NF; i++) {
            category = category " " $i
        }
        if (category ~ /^MALLOC($|_| )/) {
            malloc_bytes += $1
        }
        if (category == "IOSurface" || category ~ /^IOAccelerator/ ||
            category == "Owned physical footprint (unmapped) (graphics)") {
            graphics_bytes += $1
        }
    }
    END {
        printf "physical_malloc_bytes=%.0f\n", malloc_bytes
        printf "graphics_footprint_bytes=%.0f\n", graphics_bytes
    }
' "$output_dir/footprint-categories.txt" >>"$output_dir/summary.txt"; then
    echo "error: footprint categories could not be summarized" >&2
    exit 1
fi
cat "$output_dir/focus-summary.txt" >>"$output_dir/summary.txt"
cat "$output_dir/summary.txt"

if ! stop_app; then
    exit 1
fi

echo "profile=$output_dir"
