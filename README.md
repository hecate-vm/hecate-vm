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
cargo run -- run /path/to/program.elf --default-syscall-score 1 --syscall-score 64=8 --syscall-score 93=2
```

Manual CMake demo build:

```bash
cmake -S examples -B build/examples -DCMAKE_TOOLCHAIN_FILE=$PWD/runtime/cmake/toolchains/riscv32-clang.cmake
cmake --build build/examples --target hello_world_rv32
cargo run -- run build/examples/hello_world_rv32.elf
```

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

| Stat                          | Description                                                                                                                                         |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cycles                        | Cumulative score based on average number of cycles[^1]. This includes cycle count for all executed instructions as well as for every memory access. |
| Memory Access Count           | The total amount memory is requested from an address. This includes values that are already cached.                                                 |
| Cache hits (L1D, L1I, L2, L3) | How often each cache was hit.                                                                                                                       |

---

## Sources
[^1]: [IT Hare: Infographics: Operation Costs in CPU Clock Cycles](http://ithare.com/infographics-operation-costs-in-cpu-clock-cycles/)
