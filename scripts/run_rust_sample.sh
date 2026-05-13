#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE_DIR="$ROOT_DIR/examples/rust_hello"
MANIFEST_PATH="$EXAMPLE_DIR/Cargo.toml"
TARGET_TRIPLE="riscv32im-hecate-none-elf"
TARGET_PATH="$ROOT_DIR/runtime/rust/targets"
TARGET_SPEC="$TARGET_PATH/$TARGET_TRIPLE.json"
BIN_NAME="rust_hello"
ELF_FILE="$EXAMPLE_DIR/target/$TARGET_TRIPLE/release/$BIN_NAME.elf"

if [[ ! -f "$MANIFEST_PATH" ]]; then
  echo "Missing Rust sample manifest: $MANIFEST_PATH" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required but was not found in PATH." >&2
  exit 1
fi

if [[ ! -f "$TARGET_SPEC" ]]; then
  echo "Missing custom target file: $TARGET_SPEC" >&2
  exit 1
fi

export RUST_TARGET_PATH="$TARGET_PATH"

echo "Building Rust sample ($BIN_NAME) for $TARGET_TRIPLE..."

# Custom target specs currently require nightly + unstable rustc options.
if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup is required for custom target builds (nightly toolchain)." >&2
  exit 1
fi

if ! rustup toolchain list | grep -q '^nightly'; then
  echo "Installing nightly toolchain..."
  rustup toolchain install nightly
fi

if ! rustup component list --toolchain nightly | grep -q '^rust-src.*installed'; then
  echo "Installing rust-src for nightly..."
  rustup component add rust-src --toolchain nightly
fi

RUSTFLAGS="${RUSTFLAGS:-} -Zunstable-options" \
  cargo +nightly build \
    -Z unstable-options \
    -Z json-target-spec \
    -Z build-std=core,alloc,compiler_builtins \
    --manifest-path "$MANIFEST_PATH" \
    --release \
    --target "$TARGET_SPEC"

if [[ ! -f "$ELF_FILE" ]]; then
  # When --target is passed as JSON path, Cargo still usually uses the stem
  # as target directory, but we resolve dynamically for portability.
  ELF_FILE="$(find "$EXAMPLE_DIR/target" -type f -path "*/release/$BIN_NAME.elf" | head -n1)"
  if [[ -z "$ELF_FILE" ]]; then
    echo "Build completed but Rust ELF was not found under $EXAMPLE_DIR/target" >&2
    exit 1
  fi
fi

echo "Built sample ELF: $ELF_FILE"
cd "$ROOT_DIR"
cargo run --release -- run "$ELF_FILE" "$@"
