#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d)"
ASM_FILE="$WORK_DIR/sample.s"
ELF_FILE="$WORK_DIR/sample.elf"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

cat >"$ASM_FILE" <<'ASM'
.section .text
.globl _start
_start:
  li a0, 42
  li a7, 93
  ecall
ASM

compile_with_riscv_gcc() {
  local cc="$1"
  "$cc" \
    -nostdlib \
    -march=rv32im \
    -mabi=ilp32 \
    -Wl,-e,_start \
    -Wl,-Ttext=0x10000000 \
    -o "$ELF_FILE" \
    "$ASM_FILE"
}

compile_with_clang() {
  clang \
    --target=riscv32-unknown-elf \
    -nostdlib \
    -march=rv32im \
    -mabi=ilp32 \
    -Wl,-e,_start \
    -Wl,-Ttext=0x10000000 \
    -fuse-ld=lld \
    -o "$ELF_FILE" \
    "$ASM_FILE"
}

if command -v riscv32-none-elf-gcc >/dev/null 2>&1; then
  compile_with_riscv_gcc riscv32-none-elf-gcc
elif command -v riscv64-unknown-elf-gcc >/dev/null 2>&1; then
  compile_with_riscv_gcc riscv64-unknown-elf-gcc
elif command -v clang >/dev/null 2>&1; then
  compile_with_clang
else
  echo "No supported RISC-V compiler found." >&2
  echo "Install one of: riscv32-none-elf-gcc, riscv64-unknown-elf-gcc, or clang+lld." >&2
  exit 1
fi

echo "Built sample ELF: $ELF_FILE"
cd "$ROOT_DIR"
cargo run -- run "$ELF_FILE" --dump-registers "$@"
