#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d)"
EXAMPLES_DIR="$ROOT_DIR/examples/hello_world"
RUNTIME_DIR="$ROOT_DIR/runtime"
TOOLCHAIN_DIR="$RUNTIME_DIR/cmake/toolchains"
BUILD_DIR="$WORK_DIR/examples-build"
ELF_FILE="$BUILD_DIR/hello_world.elf"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

if [[ ! -f "$EXAMPLES_DIR/CMakeLists.txt" ]]; then
  echo "Missing demo CMake file: $EXAMPLES_DIR/CMakeLists.txt" >&2
  exit 1
fi

if [[ ! -f "$RUNTIME_DIR/CMakeLists.txt" ]]; then
  echo "Missing runtime CMake file: $RUNTIME_DIR/CMakeLists.txt" >&2
  exit 1
fi

if [[ ! -d "$TOOLCHAIN_DIR" ]]; then
  echo "Missing runtime toolchain directory: $TOOLCHAIN_DIR" >&2
  exit 1
fi

if command -v riscv32-none-elf-gcc >/dev/null 2>&1; then
  TOOLCHAIN_FILE="$TOOLCHAIN_DIR/riscv32-none-elf-gcc.cmake"
elif command -v riscv64-unknown-elf-gcc >/dev/null 2>&1; then
  TOOLCHAIN_FILE="$TOOLCHAIN_DIR/riscv64-unknown-elf-gcc.cmake"
elif command -v clang >/dev/null 2>&1; then
  TOOLCHAIN_FILE="$TOOLCHAIN_DIR/riscv32-clang.cmake"
else
  echo "No supported RISC-V compiler found." >&2
  echo "Install one of: riscv32-none-elf-gcc, riscv64-unknown-elf-gcc, or clang+lld." >&2
  exit 1
fi

cmake \
  -S "$EXAMPLES_DIR" \
  -B "$BUILD_DIR" \
  -DCMAKE_TOOLCHAIN_FILE="$TOOLCHAIN_FILE"

cmake \
  --build "$BUILD_DIR" \
  --target hello_world

echo "Built sample ELF: $ELF_FILE"
cd "$ROOT_DIR"
cargo run --release -- run "$ELF_FILE" "$@"
