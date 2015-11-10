#!/bin/sh

set -e
set -u

# Absolute path to this script, e.g. /home/user/bin/foo.sh
SCRIPT=$(readlink -f "$0")
# Absolute path this script is in, thus /home/user/bin
SCRIPT_DIR=$(dirname "$SCRIPT")
echo "$SCRIPT_DIR"

RUST_INSTALL_DIR="$WORKSPACE/install/rust"
"$SCRIPT_DIR/rustup.sh" --prefix="$RUST_INSTALL_DIR" --disable-sudo --yes
export PATH="$RUST_INSTALL_DIR/bin:$PATH"
export LD_LIBRARY_PATH="$RUST_INSTALL_DIR/lib:$LD_LIBRARY_PATH"

rustc --version
cargo --version

cargo build
cargo test
