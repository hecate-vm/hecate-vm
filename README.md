# Hecate Virtual Machine

This project provides an implementation of the Hecate virtual machine.

- [Where does the name come from?](#where-does-the-name-come-from)
- [ATTENTION](#attention)
- [Features](#features)
- [Getting Started](#getting-started)
  - [Prerequisites](#prerequisites)
- [Usage](#usage)
- [Performance Tracking](#performance-tracking)
- [Registers](#registers)
- [Bytecode Instruction Set](#bytecode-instruction-set)
- [Caches and Memory Access](#caches-and-memory-access)
  - [Caches](#caches)
  - [Memory Access](#memory-access)
    - [Write Access](#write-access)
    - [Read Access](#read-access)
      - [Prefetching and Branch Prediction](#prefetching-and-branch-prediction)
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

- Bytecode Execution:
  - Supports a well-defined set of instructions for computation, memory operations, and control flow.
  - Instructions include:
    - Arithmetic operations (Add, Sub, Mul, Div)
    - Memory access (LoadValue, LoadMemory, Store)
    - Stack manipulation (PushValue, PushReg, Pop)
    - Control flow (Call, Ret, Jmp)
    - Debugging (Inspect)
    - Halting (Halt)

- Performance Metrics:
  - Tracks the number of cycles executed.
  - Measures memory access performance using a multi-level cache scoring system (L1, L2, L3).

- Debugging and Verbose Mode:
  - **NOT YET IMPLEMENTED** Provides step-by-step debugging modes (StepOver, StepInto, StepOut) with support for code and data breakpoints.
  - Outputs detailed logs of executed instructions in verbose mode.

- Memory Model:
  - Flat memory structure with stack management.
  - Supports configurable memory size and register count.

---

## Getting Started

### Prerequisites

- Rust (for building the project)

---

## Usage

---
## Performance Tracking
The CPU tracks performance using the following stats:

| Stat                          | Description                                                                                                                                         |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cycles                        | Cumulative score based on average number of cycles[^1]. This includes cycle count for all executed instructions as well as for every memory access. |
| Memory Access Count           | The total amount memory is requested from an address. This includes values that are already cached.                                                 |
| Cache hits (L1D, L1I, L2, L3) | How often each cache was hit.                                                                                                                       |

---
## Registers
When used as a library, the CPU can be instantiated with as many registers as desired.

When using the pre-made binary, the CPU has 31 general purpose registers. R1-R31.
There is als a register called R0 which ignores writes and will always return 0.

---
## Bytecode Instruction Set
For simplicity, this implementation operates on 32 bit unsigned integers.
This might be changed later, but for now this should be enough.

**Important: the cycle count mentioned here does not include memory access and is just for the instruction itself. For memory access cycle counts see the section below**

| Opcode     | Mnemonic          | Description                                                                                        | Cycles                       |
| ---------- | ----------------- | -------------------------------------------------------------------------------------------------- | ---------------------------- |
| 0x00       | Nop               | No operation                                                                                       | 1                            |
| 0x01       | LoadValue         | Load immediate value into a register                                                               | 1                            |
| 0x02       | LoadMemory        | Load value from memory into a register                                                             | 1                            |
| 0x03       | LoadReg           | Load value from one register into another register                                                 | 1                            |
| 0x04       | LoadFromRegMemory | Load value from memory into a register, where the address is read from a register                  | 1                            |
| 0x05       | Store             | Store a register's value into memory                                                               | 1                            |
| 0x06       | StoreValue        | Store an immediate value into memory                                                               | 1                            |
| 0x07       | StoreAtReg        | Store a register's value into memory, where the address is read from a register                    | 1                            |
| 0x08       | StoreValueAtReg   | Store an immediate value into memory, where the address is read from a register                    | 1                            |
| 0x09       | PushValue         | Push immediate value onto the stack                                                                | 1                            |
| 0x0a       | PushReg           | Push register value onto the stack                                                                 | 1                            |
| 0x0b       | Pop               | Pop a value from the stack into a register                                                         | 1                            |
| 0x11       | Add               | Add two register values                                                                            | 1                            |
| 0x12       | AddValue          | Add an immediate value to a register                                                               | 1                            |
| 0x13       | FAdd              | Add two floating-point register values                                                             | 2                            |
| 0x14       | FAddValue         | Add an immediate floating-point value to a register                                                | 2                            |
| 0x15       | Sub               | Subtract one register value from another                                                           | 1                            |
| 0x16       | SubValue          | Subtract an immediate value from a register                                                        | 1                            |
| 0x17       | FSub              | Subtract one floating-point register value from another                                            | 2                            |
| 0x18       | FSubValue         | Subtract an immediate floating-point value from a register                                         | 2                            |
| 0x19       | Mul               | Multiply two register values                                                                       | 4                            |
| 0x1A       | MulValue          | Multiply a register value by an immediate value                                                    | 4                            |
| 0x1B       | FMul              | Multiply two floating-point register values                                                        | 4                            |
| 0x1C       | FMulValue         | Multiply a register value by an immediate floating-point value                                     | 4                            |
| 0x1D       | Div               | Divide one register value by another                                                               | 27                           |
| 0x1E       | DivValue          | Divide a register value by an immediate value                                                      | 27                           |
| 0x1F       | FDiv              | Divide one floating-point register value by another                                                | 27                           |
| 0x20       | FDivValue         | Divide a register's floating-point value by an immediate value                                     | 27                           |
| 0x70       | And               | Binary AND two registers                                                                           | 1                            |
| 0x71       | AndValue          | Binary AND a register and an immediate value                                                       | 1                            |
| 0x72       | Or                | Binary OR two registers                                                                            | 1                            |
| 0x73       | OrValue           | Binary OR a register and an immediate value                                                        | 1                            |
| 0x74       | Xor               | Binary XOR two registers                                                                           | 1                            |
| 0x75       | XorValue          | Binary XOR a register and an immediate value                                                       | 1                            |
| 0x76       | Not               | Binary NOT a register                                                                              | 1                            |
| 0x77       | ShiftLeft         | Shifts the value in the first register to the left by the value in the second register             | 1                            |
| 0x78       | ShiftLeftValue    | Shifts the value in the first register to the left by an immediate value                           | 1                            |
| 0x79       | ShiftRight        | Shifts the value in the first register to the right by the value in the second register            | 1                            |
| 0x7a       | ShiftRightValue   | Shifts the value in the first register to the right by an immediate value                          | 1                            |
| 0xB0       | LoadByte          | Load a byte from memory into a register                                                            | 2                            |
| 0xB1       | StoreByte         | Store the least significant byte of a register into memory                                         | 2                            |
| 0xC00      | Jmp               | Unconditional jump to a specific address                                                           | 1                            |
| 0xC01      | Cmp               | Compare two register values                                                                        | 1                            |
| 0xC02      | CmpValue          | Compare a register value with an immediate value                                                   | 1                            |
| 0xC03      | FCmp              | Compare two floating-point register values                                                         | 2                            |
| 0xC04      | FCmpValue         | Compare a register's floating-point value with an immediate value                                  | 2                            |
| 0xC05      | Je                | Jump if equal (Zero Flag is set)                                                                   | 2                            |
| 0xC06      | Jne               | Jump if not equal (Zero Flag is not set)                                                           | 2                            |
| 0xC07      | Jg                | Jump if greater (Zero Flag not set and Sign Flag equals Overflow Flag)                             | 2                            |
| 0xC08      | Jge               | Jump if greater or equal (Sign Flag equals Overflow Flag)                                          | 2                            |
| 0xC09      | Jl                | Jump if less (Sign Flag not equal to Overflow Flag)                                                | 2                            |
| 0xC0A      | Jle               | Jump if less or equal (Zero Flag is set or Sign Flag not equal to Overflow Flag)                   | 2                            |
| 0xC0B      | Ja                | Jump if above (Carry Flag not set and Zero Flag not set)                                           | 2                            |
| 0xC0C      | Jae               | Jump if above or equal (Carry Flag not set)                                                        | 2                            |
| 0xC0D      | Jb                | Jump if below (Carry Flag is set)                                                                  | 2                            |
| 0xC0E      | Jbe               | Jump if below or equal (Carry Flag is set or Zero Flag is set)                                     | 2                            |
| 0xC0F      | Jc                | Jump if carry (Carry Flag is set)                                                                  | 2                            |
| 0xC10      | Jnc               | Jump if no carry (Carry Flag is not set)                                                           | 2                            |
| 0xC11      | Jo                | Jump if overflow (Overflow Flag is set)                                                            | 2                            |
| 0xC12      | Jno               | Jump if no overflow (Overflow Flag is not set)                                                     | 2                            |
| 0xC13      | Js                | Jump if sign (Sign Flag is set)                                                                    | 2                            |
| 0xC14      | Jns               | Jump if no sign (Sign Flag is not set)                                                             | 2                            |
| 0xCFF      | Jxcz              | Jump if CX register is zero (ignores flags)                                                        | 2                            |
| 0xF0       | Call              | Call a function by pushing return address and jumping                                              | 25                           |
| 0xF1       | Ret               | Return from a function by popping the return address                                               | 5                            |
| 0xF2       | Syscall           | Perform a system call (currently every syscall is a no-op)                                         | Variable (Depends on HostIO) |
| 0xFFFFFFF0 | Inspect           | Output the value of a memory address (special instruction for debugging, therefore: no cycle cost) | 0                            |
| 0xFFFFFFFF | Halt              | Halt the program execution                                                                         | 0                            |


---
## Caches and Memory Access
### Caches

Caching in this VM is simulated. This means the VM never actually copies values into a cache,
it just remembers which memory locations are in which cache.

The VM uses hierarchical caching (L1, L2, L3) and has separate L1 caches for data (L1D) and instructions (L2D).

### Memory Access
#### Write Access
In real hardware the CPU would write to cache and the hardware would eventually write back that value to main memory.
As this process does not stall the CPU (AFAIK) the VM just assumes a cycle cost of 1.

#### Read Access
- L1 Cache Hit: 3 Cycles
- L2 Cache Hit: 11 Cycles
- L3 Cache Hit: 50 Cycles
- Main Memory: 125 Cycles

##### Prefetching and Branch Prediction
There is currently a simple prefetcher which fetches 2 cache lines of data on access and 10 lines of instructions.
There is currently no branch predictor. Once there is, it will affect how prefetching works.

---

## Sources
[^1]: [IT Hare: Infographics: Operation Costs in CPU Clock Cycles](http://ithare.com/infographics-operation-costs-in-cpu-clock-cycles/)
