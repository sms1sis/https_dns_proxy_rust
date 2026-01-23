#!/bin/bash
set -e

# Configuration
TOOLCHAIN_DIR="$(pwd)/toolchains"
RUSTUP_DIR="$(pwd)/local-rust"
LINKER_URL="http://musl.cc/mips-linux-musl-cross.tgz"
LINKER_NAME="mips-linux-musl-cross"

echo "Setting up cross-compilation environment..."

# 1. Setup Linker (C Toolchain)
mkdir -p "$TOOLCHAIN_DIR"
if [ ! -d "$TOOLCHAIN_DIR/$LINKER_NAME" ]; then
    echo "Downloading MIPS Linker..."
    curl -L "$LINKER_URL" -o "$TOOLCHAIN_DIR/linker.tgz"
    echo "Extracting Linker..."
    tar -xf "$TOOLCHAIN_DIR/linker.tgz" -C "$TOOLCHAIN_DIR"
    rm "$TOOLCHAIN_DIR/linker.tgz"
else
    echo "Linker already present."
fi

# Path to the specific-gcc
LINKER_BIN="$TOOLCHAIN_DIR/$LINKER_NAME/bin/mips-linux-musl-gcc"

# 2. Setup Local Rustup (if system rust cannot add targets)
# We check if we can add targets to system rust, if not we use local.
# Given previous checks, we assume local is needed.

export RUSTUP_HOME="$RUSTUP_DIR/rustup"
export CARGO_HOME="$RUSTUP_DIR/cargo"
mkdir -p "$RUSTUP_DIR"

if [ ! -f "$CARGO_HOME/bin/cargo" ]; then
    echo "Installing local Rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain stable
else
    echo "Local Rustup already present."
fi

# Use local cargo/rustc
export PATH="$CARGO_HOME/bin:$PATH"

echo "Adding MIPS target..."
rustup target add mips-unknown-linux-musl

# 3. Configure Cargo
echo "Configuring .cargo/config.toml..."
mkdir -p .cargo
cat > .cargo/config.toml <<EOF
[target.mips-unknown-linux-musl]
linker = "$LINKER_BIN"
rustflags = ["-C", "strip=symbols"]

[build]
target = "mips-unknown-linux-musl"
EOF

echo "Environment setup complete."
echo "To build, run:"
echo "export RUSTUP_HOME=\"$RUSTUP_DIR/rustup\""
echo "export CARGO_HOME=\"$RUSTUP_DIR/cargo\""
echo "export PATH=\" জৈ$CARGO_HOME/bin:$PATH\""
echo "cargo build --profile release-mips"
