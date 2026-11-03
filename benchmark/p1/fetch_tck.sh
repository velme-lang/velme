#!/bin/sh
# Sparse-clone the DMN TCK (Apache-2.0) at a pinned commit into .cache/tck. Idempotent. Not vendored.
set -eu
COMMIT=0456e1a2fa5b9c2382cc7380cc6b95f0031e4793
DIR="$(cd "$(dirname "$0")" && pwd)/.cache/tck"
L3=TestCases/compliance-level-3
CASES="$L3/0087-chapter-11-example $L3/0020-vacation-days $L3/0014-loan-comparison"
ready=yes
[ "$(git -C "$DIR" rev-parse HEAD 2>/dev/null)" = "$COMMIT" ] || ready=no
for c in $CASES; do
    [ -f "$DIR/$c/$(basename "$c").dmn" ] || ready=no
done
if [ "$ready" = yes ]; then
    echo "TCK already at $COMMIT"; exit 0
fi
rm -rf "$DIR"; mkdir -p "$DIR"
git -C "$DIR" init -q
git -C "$DIR" remote add origin https://github.com/dmn-tck/tck.git
# shellcheck disable=SC2086 # CASES is a space-separated list of paths without spaces
git -C "$DIR" sparse-checkout set $CASES
git -C "$DIR" fetch -q --depth 1 --filter=blob:none origin "$COMMIT"
git -C "$DIR" checkout -q FETCH_HEAD
echo "TCK at $(git -C "$DIR" rev-parse HEAD)"
