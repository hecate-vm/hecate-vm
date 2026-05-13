cmake_minimum_required(VERSION 3.20)
project(hecate_runtime C)

list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake")
include(HecateRuntime)

add_library(hecate_runtime_headers INTERFACE)
target_include_directories(hecate_runtime_headers
  INTERFACE
    ${CMAKE_CURRENT_SOURCE_DIR}/include
)

add_library(hecate_runtime_rv32 OBJECT
  src/crt0_rv32.c
  src/syscalls_rv32.c
  src/stdio_rv32.c
)

target_link_libraries(hecate_runtime_rv32
  PUBLIC
    hecate_runtime_headers
)

hecate_runtime_apply_rv32_compile_options(hecate_runtime_rv32)
