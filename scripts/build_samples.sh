#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="target/hecate-rv32-build"
EXAMPLES_ROOT="$ROOT_DIR/examples"
RUNTIME_DIR="$ROOT_DIR/runtime"
TOOLCHAIN_DIR="$RUNTIME_DIR/cmake/toolchains"
BUILD_ROOT="$WORK_DIR/examples-build"

if [[ ! -d "$EXAMPLES_ROOT" ]]; then
  echo "Missing examples directory: $EXAMPLES_ROOT" >&2
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

rm -rf "$BUILD_ROOT"
mkdir -p "$BUILD_ROOT"

built_elf_count=0

for example_dir in "$EXAMPLES_ROOT"/*; do
  if [[ ! -d "$example_dir" ]]; then
    continue
  fi

  if [[ ! -f "$example_dir/CMakeLists.txt" ]]; then
    continue
  fi

  example_name="$(basename "$example_dir")"
  example_build_dir="$BUILD_ROOT/$example_name"

  echo "Configuring example: $example_name"
  cmake \
    -S "$example_dir" \
    -B "$example_build_dir" \
    -DCMAKE_TOOLCHAIN_FILE="$TOOLCHAIN_FILE"

  echo "Building example: $example_name"
  cmake --build "$example_build_dir"

  while IFS= read -r elf_path; do
    echo "Built ELF: $elf_path"
    built_elf_count=$((built_elf_count + 1))
  done < <(find "$example_build_dir" -type f -name '*.elf' | sort)
done

if [[ "$built_elf_count" -eq 0 ]]; then
  echo "No .elf files were built under $BUILD_ROOT" >&2
  exit 1
fi

echo "Total ELF files built: $built_elf_count"