#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="target/hecate-rv32-build"
EXAMPLES_ROOT="$ROOT_DIR/examples"
RUNTIME_DIR="$ROOT_DIR/runtime"
TOOLCHAIN_DIR="$RUNTIME_DIR/cmake/toolchains"
BUILD_ROOT="$WORK_DIR/examples-build"
RUST_TARGET_TRIPLE="riscv32im-hecate-none-elf"
RUST_TARGET_PATH="$ROOT_DIR/runtime/rust/targets"
RUST_TARGET_SPEC="$RUST_TARGET_PATH/$RUST_TARGET_TRIPLE.json"

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
found_rust_example=0
SELECTED_EXAMPLES=()
if [[ -n "${HECATE_EXAMPLES:-}" ]]; then
  IFS=',' read -r -a SELECTED_EXAMPLES <<< "$HECATE_EXAMPLES"
fi

should_build_example() {
  local example_name="$1"

  if [[ "${#SELECTED_EXAMPLES[@]}" -eq 0 ]]; then
    return 0
  fi

  for selected in "${SELECTED_EXAMPLES[@]}"; do
    if [[ "$selected" == "$example_name" ]]; then
      return 0
    fi
  done

  return 1
}

ensure_rust_nightly() {
  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo is required to build Rust examples." >&2
    exit 1
  fi

  if ! command -v rustup >/dev/null 2>&1; then
    echo "rustup is required for Rust custom-target builds." >&2
    exit 1
  fi

  if ! rustup toolchain list | grep -q '^nightly'; then
    echo "Installing nightly toolchain for Rust examples..."
    rustup toolchain install nightly
  fi

  if ! rustup component list --toolchain nightly | grep -q '^rust-src.*installed'; then
    echo "Installing rust-src for nightly..."
    rustup component add rust-src --toolchain nightly
  fi

  if [[ ! -f "$RUST_TARGET_SPEC" ]]; then
    echo "Missing custom target file: $RUST_TARGET_SPEC" >&2
    exit 1
  fi
}

build_cmake_example() {
  local example_dir="$1"
  local example_name="$2"
  local example_build_dir="$BUILD_ROOT/$example_name"
  local example_var_name
  local example_args_var
  local -a cmake_args

  example_var_name="$(echo "$example_name" | tr '[:lower:]-' '[:upper:]_')"
  example_args_var="HECATE_CMAKE_ARGS_${example_var_name}"

  cmake_args=(
    -S "$example_dir"
    -B "$example_build_dir"
    -DCMAKE_TOOLCHAIN_FILE="$TOOLCHAIN_FILE"
  )

  if [[ -n "${HECATE_CMAKE_ARGS:-}" ]]; then
    # shellcheck disable=SC2206
    cmake_args+=( ${HECATE_CMAKE_ARGS} )
  fi

  if [[ -n "${!example_args_var:-}" ]]; then
    # shellcheck disable=SC2206
    cmake_args+=( ${!example_args_var} )
  fi

  echo "Configuring C/C++ example: $example_name"
  cmake "${cmake_args[@]}"

  echo "Building C/C++ example: $example_name"
  cmake --build "$example_build_dir"

  while IFS= read -r elf_path; do
    echo "Built ELF: $elf_path"
    built_elf_count=$((built_elf_count + 1))
  done < <(find "$example_build_dir" -type f -name '*.elf' | sort)
}

build_rust_example() {
  local example_dir="$1"
  local example_name="$2"
  local rust_out_dir="$BUILD_ROOT/$example_name"

  if [[ "$found_rust_example" -eq 0 ]]; then
    ensure_rust_nightly
    found_rust_example=1
  fi

  echo "Building Rust example: $example_name"
  (
    cd "$ROOT_DIR"
    RUST_TARGET_PATH="$RUST_TARGET_PATH" \
    RUSTFLAGS="${RUSTFLAGS:-} -Zunstable-options" \
      cargo +nightly build \
        -Z unstable-options \
        -Z json-target-spec \
        -Z build-std=core,alloc,compiler_builtins \
        --manifest-path "$example_dir/Cargo.toml" \
        --release \
        --target "$RUST_TARGET_SPEC"
  )

  mkdir -p "$rust_out_dir"
  while IFS= read -r built_elf; do
    out_name="$(basename "$built_elf")"
    cp -f "$built_elf" "$rust_out_dir/$out_name"
    echo "Built ELF: $rust_out_dir/$out_name"
    built_elf_count=$((built_elf_count + 1))
  done < <(find "$example_dir/target/$RUST_TARGET_TRIPLE/release" -maxdepth 1 -type f -name '*.elf' | sort)
}

for example_dir in "$EXAMPLES_ROOT"/*; do
  if [[ ! -d "$example_dir" ]]; then
    continue
  fi

  example_name="$(basename "$example_dir")"

  if ! should_build_example "$example_name"; then
    continue
  fi

  if [[ -f "$example_dir/CMakeLists.txt" ]]; then
    build_cmake_example "$example_dir" "$example_name"
  fi

  if [[ -f "$example_dir/Cargo.toml" ]]; then
    build_rust_example "$example_dir" "$example_name"
  fi
done

if [[ "$built_elf_count" -eq 0 ]]; then
  echo "No .elf files were built under $BUILD_ROOT" >&2
  exit 1
fi

echo "Total ELF files built: $built_elf_count"