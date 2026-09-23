#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-D pub-use-of-private-extern-crate" \
    cargo check --workspace --locked
