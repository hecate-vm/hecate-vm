#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d)"
C_FILE="$ROOT_DIR/examples/hello_world_rv32.c"
RUNTIME_DIR="$ROOT_DIR/runtime"
RUNTIME_INCLUDE_DIR="$RUNTIME_DIR/include"
CRT_FILE="$RUNTIME_DIR/src/crt0_rv32.c"
SYSCALLS_FILE="$RUNTIME_DIR/src/syscalls_rv32.c"
STDIO_FILE="$RUNTIME_DIR/src/stdio_rv32.c"
ELF_FILE="$WORK_DIR/sample.elf"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

if [[ ! -f "$C_FILE" ]]; then
  echo "Missing demo source file: $C_FILE" >&2
  exit 1
fi

if [[ ! -f "$CRT_FILE" ]]; then
  echo "Missing runtime source file: $CRT_FILE" >&2
  exit 1
fi

if [[ ! -f "$SYSCALLS_FILE" ]]; then
  echo "Missing runtime source file: $SYSCALLS_FILE" >&2
  exit 1
fi

if [[ ! -f "$STDIO_FILE" ]]; then
  echo "Missing runtime source file: $STDIO_FILE" >&2
  exit 1
fi

if [[ ! -d "$RUNTIME_INCLUDE_DIR" ]]; then
  echo "Missing runtime include directory: $RUNTIME_INCLUDE_DIR" >&2
  exit 1
fi

compile_with_riscv_gcc() {
  local cc="$1"
  "$cc" \
    -nostdlib \
    -ffreestanding \
    -fno-builtin \
    -march=rv32im \
    -mabi=ilp32 \
    -I"$RUNTIME_INCLUDE_DIR" \
    -Wl,-e,_start \
    -Wl,-Ttext=0x10000000 \
    -o "$ELF_FILE" \
    "$CRT_FILE" \
    "$SYSCALLS_FILE" \
    "$STDIO_FILE" \
    "$C_FILE"
}

compile_with_clang() {
  clang \
    --target=riscv32-unknown-elf \
    -nostdlib \
    -ffreestanding \
    -fno-builtin \
    -march=rv32im \
    -mabi=ilp32 \
    -I"$RUNTIME_INCLUDE_DIR" \
    -Wl,-e,_start \
    -Wl,-Ttext=0x10000000 \
    -fuse-ld=lld \
    -o "$ELF_FILE" \
    "$CRT_FILE" \
    "$SYSCALLS_FILE" \
    "$STDIO_FILE" \
    "$C_FILE"
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
