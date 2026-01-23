#!/bin/bash
set -e

RUSTUP_DIR="$(pwd)/local-rust"
TOOLCHAIN_DIR="$(pwd)/toolchains"
LINKER_NAME="mips-linux-musl-cross"

# Paths to tools
GCC_PATH="$TOOLCHAIN_DIR/$LINKER_NAME/bin/mips-linux-musl-gcc"
GXX_PATH="$TOOLCHAIN_DIR/$LINKER_NAME/bin/mips-linux-musl-g++"
AR_PATH="$TOOLCHAIN_DIR/$LINKER_NAME/bin/mips-linux-musl-ar"

export RUSTUP_HOME="$RUSTUP_DIR/rustup"
export CARGO_HOME="$RUSTUP_DIR/cargo"
export PATH="$CARGO_HOME/bin:$PATH"

# Set CC for ring and other C-based crates
export CC_mips_unknown_linux_musl="$GCC_PATH"
export CXX_mips_unknown_linux_musl="$GXX_PATH"
export AR_mips_unknown_linux_musl="$AR_PATH"

echo "Switching to Nightly for build-std support..."
# Only install if not already there (though rustup handles idempotency well)
rustup toolchain install nightly
rustup default nightly
rustup component add rust-src

echo "Nightly setup complete."
echo "To build, use:"
echo "export RUSTUP_HOME=\"$RUSTUP_DIR/rustup\""
echo "export CARGO_HOME=\"$RUSTUP_DIR/cargo\""
echo "export PATH=\"\$CARGO_HOME/bin:\$PATH\""
echo "export CC_mips_unknown_linux_musl=\"$GCC_PATH\""
echo "export CXX_mips_unknown_linux_musl=\"$GXX_PATH\""
echo "export AR_mips_unknown_linux_musl=\"$AR_PATH\""
echo "cargo build -Z build-std=std,panic_abort --target mips-unknown-linux-musl --profile release-mips"