#!/usr/bin/env bash
# Build the Noir->Groth16 backend ported to noir beta.22. Clones upstream at the
# pinned commit, applies beta22-port.patch, and builds noir-cli. See README.md.
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
SRC="$DIR/build"
UPSTREAM="https://github.com/jamesbachini/Noir-Groth16"
COMMIT="4b7caace1f2128e454c8d0fe50cac1ec46b1e272"

if [ ! -d "$SRC/.git" ]; then
  git clone "$UPSTREAM" "$SRC"
fi
cd "$SRC"
git checkout -q -- .
git checkout -q "$COMMIT"
git apply "$DIR/beta22-port.patch"
rm -f Cargo.lock
cargo build --release -p noir-cli
echo "OK: $SRC/target/release/noir-cli"
