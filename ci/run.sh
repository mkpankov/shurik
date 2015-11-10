#!/bin/sh

set -e

RUST_INSTALL_DIR="$WORKSPACE/install/rust"
./rustup.sh --prefix="$RUST_INSTALL_DIR" --disable-sudo --yes
export PATH="RUST_INSTALL_DIR/bin:$PATH"
export LD_LIBRARY_PATH="RUST_INSTALL_DIR/lib:$LD_LIBRARY_PATH"

rustc --version
cargo --version

cargo build
cargo test
