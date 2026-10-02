#!/bin/sh
# Sparse-clone the DMN TCK (Apache-2.0) at a pinned commit into .cache/tck. Idempotent. Not vendored.
set -eu
COMMIT=0456e1a2fa5b9c2382cc7380cc6b95f0031e4793
DIR="$(cd "$(dirname "$0")" && pwd)/.cache/tck"
CASE=TestCases/compliance-level-3/0087-chapter-11-example
if [ -f "$DIR/$CASE/0087-chapter-11-example.dmn" ] && [ "$(git -C "$DIR" rev-parse HEAD 2>/dev/null)" = "$COMMIT" ]; then
    echo "TCK already at $COMMIT"; exit 0
fi
rm -rf "$DIR"; mkdir -p "$DIR"
git -C "$DIR" init -q
git -C "$DIR" remote add origin https://github.com/dmn-tck/tck.git
git -C "$DIR" sparse-checkout set "$CASE"
git -C "$DIR" fetch -q --depth 1 --filter=blob:none origin "$COMMIT"
git -C "$DIR" checkout -q FETCH_HEAD
echo "TCK at $(git -C "$DIR" rev-parse HEAD)"
