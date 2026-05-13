# Hecate Virtual Machine

Quick run:

```bash
./scripts/run_sample_rv32.sh
```

- [Where does the name come from?](#where-does-the-name-come-from)
- [ATTENTION](#attention)
- [Features](#features)
- [Getting Started](#getting-started)
  - [Prerequisites](#prerequisites)
- [Usage](#usage)
- [Performance Tracking](#performance-tracking)
- [Sources](#sources)

---

## Where does the name come from?

The project is named Hecate, inspired by the ancient Greek goddess associated with magic, crossroads, and guiding transformations.
Just as Hecate stood at the intersection of possibilities, this CPU project represents the convergence of computational logic,
optimization, and problem-solving, providing participants with a structured yet flexible virtual machine to explore and
optimize their solutions.

The name reflects both the mystical allure of programming challenges and the power of guiding complex tasks to completion.

---

## ATTENTION
**This project is currently unstable. Changes are to be expected. Use at your own risk!**

---

## Features

- Input: RV32 ELF executable
- Execution core: `rvsim` interpreter
- Hecate additions: cache-aware memory tracking and cycle score reporting

---

## Getting Started

### Prerequisites

- Rust (for building the project)
- CMake + Clang (for building the sample code)

---

## Usage

Useful options:

```bash
cargo run -- run /path/to/program.elf --dump-registers
cargo run -- run /path/to/program.elf --max-instructions 1000000
cargo run -- run /path/to/program.elf --cache-line-size 64 --l1-size 32768 --l2-size 262144 --l3-size 8388608
cargo run -- run /path/to/program.elf --config /path/to/hecate.toml
```

The runtime always loads built-in defaults from `src/default.toml` and merges them with the file passed through `--config`.
Only the keys you provide need to be overridden.

Example config:

```toml
default_syscall_cycles = 500
io_cycles_per_byte = 20

[latency]
l1 = 3
l2 = 11
l3 = 50
memory = 125
store = 1

[syscall_cycles]
64 = 500
93 = 0
```

Manual CMake demo build:

```bash
cmake -S examples -B build/examples -DCMAKE_TOOLCHAIN_FILE=$PWD/runtime/cmake/toolchains/riscv32-clang.cmake
cmake --build build/examples --target hello_world_rv32
cargo run -- run build/examples/hello_world_rv32.elf
```

Rust demo build (Cargo cross-compile):

```bash
rustup target add riscv32im-unknown-none-elf
cargo build --manifest-path examples/rust_hello/Cargo.toml --release --target riscv32im-unknown-none-elf
cargo run -- run examples/rust_hello/target/riscv32im-unknown-none-elf/release/rust_hello
```

Rust runtime helper crate:

- `runtime/rust/hecate_runtime/` (no_std helpers for write/exit + print macros)
- `examples/rust_hello/` (minimal no_std + no_main RV32 example)

If you have GCC cross-compilers installed, you can use one of these instead:

- `runtime/cmake/toolchains/riscv32-none-elf-gcc.cmake`
- `runtime/cmake/toolchains/riscv64-unknown-elf-gcc.cmake`

Reusable runtime module:

- Runtime root: `runtime/`
- CMake entrypoint: `runtime/CMakeLists.txt`
- CMake helpers: `runtime/cmake/HecateRuntime.cmake`
- Headers: `runtime/include/hecate_runtime/`

To use in another CMake project:

```cmake
add_subdirectory(path/to/hecate-vm/runtime hecate-runtime)

add_executable(my_program my_program.c)
set_target_properties(my_program PROPERTIES SUFFIX ".elf")
hecate_runtime_link(my_program)
```

---
## Performance Tracking
The CPU tracks performance using the following stats:

| Stat                          | Description                                                                                                                                                         |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cycles                        | Total charged cycles[^1]. This includes retired instructions, memory hierarchy costs, configured syscall costs, and per-byte I/O costs for writes to stdout/stderr. |
| Syscall cycles contribution   | Total cycles charged to syscalls, including variable per-byte I/O cost where applicable.                                                                            |
| I/O cycles contribution       | The subset of syscall cycles added by `io_cycles_per_byte` during writes to stdout/stderr.                                                                          |
| IO Bytes Written              | Number of bytes written through syscall `64` to stdout/stderr.                                                                                                      |
| Memory Access Count           | The total amount memory is requested from an address. This includes values that are already cached.                                                                 |
| Cache hits (L1D, L1I, L2, L3) | How often each cache was hit.                                                                                                                                       |

When syscall activity is present, the VM also prints a per-syscall breakdown with call count, configured base cycles, any variable cycles, and the final subtotal charged for that syscall.

---

## Sources
[^1]: [IT Hare: Infographics: Operation Costs in CPU Clock Cycles](http://ithare.com/infographics-operation-costs-in-cpu-clock-cycles/)
