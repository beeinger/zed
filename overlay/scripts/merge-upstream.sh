#!/usr/bin/env bash
# Fetch official Zed and merge it into the current branch, then verify FORK
# markers survived.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

upstream_remote="${UPSTREAM_REMOTE:-upstream}"
upstream_branch="${UPSTREAM_BRANCH:-main}"

if [[ -n "$(git status --porcelain)" ]]; then
    echo "worktree is not clean; commit or stash before merging upstream" >&2
    git status --short >&2
    exit 1
fi

if ! git remote get-url "$upstream_remote" >/dev/null 2>&1; then
    echo "git remote '$upstream_remote' is not configured" >&2
    echo "expected zed-industries/zed (this fork uses origin = beeinger/zed)" >&2
    exit 1
fi

git fetch "$upstream_remote" "$upstream_branch"

echo "merging ${upstream_remote}/${upstream_branch} into $(git branch --show-current)"
if ! git merge --no-edit "${upstream_remote}/${upstream_branch}"; then
    echo "merge stopped with conflicts. Resolve these first (see overlay/UPSTREAM.md):" >&2
    git diff --name-only --diff-filter=U >&2
    echo "then run overlay/scripts/check-touchpoints.sh" >&2
    exit 1
fi

"$root/overlay/scripts/check-touchpoints.sh"
echo "upstream merge complete"
