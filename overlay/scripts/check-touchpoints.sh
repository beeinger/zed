#!/usr/bin/env bash
# Verify every applied FORK marker still exists, and every FORK hunk in the
# tree is listed in overlay/TOUCHPOINTS.md.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

touchpoints="$root/overlay/TOUCHPOINTS.md"
if [[ ! -f "$touchpoints" ]]; then
    echo "missing $touchpoints" >&2
    exit 1
fi

status=0

while read -r marker path; do
    file="$root/$path"
    if [[ ! -f "$file" ]]; then
        echo "missing file for marker '$marker': $path" >&2
        status=1
        continue
    fi
    if ! grep -q "FORK:${marker}" "$file"; then
        echo "missing FORK:${marker} in $path" >&2
        status=1
    fi
done < <(awk '/^APPLIED / { print $2, $3 }' "$touchpoints")

listed_markers="$(mktemp)"
found_markers="$(mktemp)"
trap 'rm -f "$listed_markers" "$found_markers"' EXIT

awk '/^APPLIED / { print $2 }' "$touchpoints" | sort -u >"$listed_markers"

# Collect FORK:<id> from tracked and untracked source, ignoring docs/scripts
# and the closing FORK:end marker.
git grep -h -o -E 'FORK:[A-Za-z0-9_-]+' -- \
    ':!overlay/TOUCHPOINTS.md' \
    ':!overlay/README.md' \
    ':!overlay/UPSTREAM.md' \
    ':!overlay/scripts/*' \
    ':!*.md' \
    2>/dev/null | sed 's/^FORK://' | grep -v '^end$' | sort -u >"$found_markers" || true

# git grep skips untracked overlay crates; scan those too.
if [[ -d overlay/crates ]]; then
    grep -R -h -o -E 'FORK:[A-Za-z0-9_-]+' overlay/crates 2>/dev/null \
        | sed 's/^FORK://' | grep -v '^end$' | sort -u >>"$found_markers" || true
    sort -u -o "$found_markers" "$found_markers"
fi

unlisted="$(comm -13 "$listed_markers" "$found_markers" || true)"
if [[ -n "$unlisted" ]]; then
    echo "FORK markers in the tree that are not listed as APPLIED in TOUCHPOINTS.md:" >&2
    echo "$unlisted" >&2
    status=1
fi

if [[ "$status" -ne 0 ]]; then
    exit 1
fi

echo "touchpoints ok ($(wc -l <"$listed_markers" | tr -d ' ') applied markers)"
