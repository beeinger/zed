#!/usr/bin/env bash
# Build an unsigned Peekado.app (+ DMG) on macOS. Keep symbols for tester logs.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

target_triple="${1:-}"
if [[ -z "$target_triple" ]]; then
    host_line="$(rustc --version --verbose | grep '^host:')"
    target_triple="${host_line#host: }"
fi

export ZED_RELEASE_CHANNEL="${ZED_RELEASE_CHANNEL:-dev}"
export ALLOW_MISSING_LICENSES="${ALLOW_MISSING_LICENSES:-1}"
# Do not strip; testers send backtraces from Peekado.log
unset PEEKADO_STRIP || true

echo "Building Peekado for ${target_triple} (channel=${ZED_RELEASE_CHANNEL})"
./script/bundle-mac "${target_triple}"

arch_suffix=""
case "$target_triple" in
    aarch64-apple-darwin) arch_suffix="aarch64" ;;
    x86_64-apple-darwin) arch_suffix="x86_64" ;;
    *) echo "unsupported target ${target_triple}" >&2; exit 1 ;;
esac

dmg="target/${target_triple}/release/Peekado-${arch_suffix}.dmg"
if [[ -f "$dmg" ]]; then
    echo "DMG: $dmg"
fi

# Zip whatever .app cargo-bundle produced so it can be scp'd without hdiutil.
app="$(find target -path '*Peekado*.app' -prune -print | head -n 1 || true)"
if [[ -n "$app" && -d "$app" ]]; then
    zip_path="target/Peekado-macos-${arch_suffix}.zip"
    rm -f "$zip_path"
    ditto -c -k --keepParent "$app" "$zip_path"
    echo "ZIP: $zip_path"
    echo "APP: $app"
fi
