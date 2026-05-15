#!/usr/bin/env bash
# Builds and compares a random linked-list traversal (naive) against a
# contiguous vector scan (data-oriented).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNTIME_DIR="$ROOT_DIR/runtime"
TOOLCHAIN_DIR="$RUNTIME_DIR/cmake/toolchains"

LIST_DIR="$ROOT_DIR/examples/linked_list"
VEC_DIR="$ROOT_DIR/examples/vector"

WORK_DIR="$(mktemp -d)"
LIST_BUILD="$WORK_DIR/list-build"
VEC_BUILD="$WORK_DIR/vector-build"
LIST_ELF="$LIST_BUILD/linked_list.elf"
VEC_ELF="$VEC_BUILD/vector.elf"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Sanity checks
# ---------------------------------------------------------------------------
for d in "$LIST_DIR" "$VEC_DIR"; do
  if [[ ! -f "$d/CMakeLists.txt" ]]; then
    echo "Missing CMakeLists.txt in $d" >&2
    exit 1
  fi
done

if [[ ! -f "$RUNTIME_DIR/CMakeLists.txt" ]]; then
  echo "Missing runtime CMake file: $RUNTIME_DIR/CMakeLists.txt" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Toolchain selection
# ---------------------------------------------------------------------------
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

# ---------------------------------------------------------------------------
# Build both examples
# ---------------------------------------------------------------------------
build_example() {
  local src_dir="$1"
  local build_dir="$2"
  local target="$3"

  cmake \
    -S "$src_dir" \
    -B "$build_dir" \
    -DCMAKE_TOOLCHAIN_FILE="$TOOLCHAIN_FILE" \
    --log-level=WARNING

  cmake --build "$build_dir" --target "$target"
}

echo "Building linked_list..."
build_example "$LIST_DIR" "$LIST_BUILD" linked_list

echo "Building vector..."
build_example "$VEC_DIR" "$VEC_BUILD" vector

# ---------------------------------------------------------------------------
# Run both through the VM and capture output
# ---------------------------------------------------------------------------
cd "$ROOT_DIR"

echo ""
echo "================================================================"
echo " Running linked_list (random linked list traversal)"
echo "================================================================"
LIST_OUT="$(cargo run --release --quiet -- run "$LIST_ELF" "$@" 2>&1)"
echo "$LIST_OUT"
LIST_CYCLES="$(echo "$LIST_OUT" | grep "Score (cycles):" | awk '{print $NF}')"

echo ""
echo "================================================================"
echo " Running vector (linear vector scan)"
echo "================================================================"
VEC_OUT="$(cargo run --release --quiet -- run "$VEC_ELF" "$@" 2>&1)"
echo "$VEC_OUT"
VEC_CYCLES="$(echo "$VEC_OUT" | grep "Score (cycles):" | awk '{print $NF}')"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "================================================================"
echo " COMPARISON SUMMARY"
echo "================================================================"
echo " Linked list cycles : $LIST_CYCLES"
echo " Vector      cycles : $VEC_CYCLES"
if [[ -n "$LIST_CYCLES" && -n "$VEC_CYCLES" && "$LIST_CYCLES" -gt 0 ]]; then
  DELTA=$(( (LIST_CYCLES - VEC_CYCLES) * 100 / LIST_CYCLES ))
  if (( VEC_CYCLES < LIST_CYCLES )); then
    echo " Vector uses ${DELTA}% fewer cycles than linked list"
  elif (( VEC_CYCLES > LIST_CYCLES )); then
    DELTA=$(( (VEC_CYCLES - LIST_CYCLES) * 100 / LIST_CYCLES ))
    echo " Vector uses ${DELTA}% more cycles than linked list"
  else
    echo " Vector and linked list use the same number of cycles"
  fi
fi
echo "================================================================"
