cmake_minimum_required(VERSION 3.20)
project(hecate_examples C)

add_subdirectory(${CMAKE_CURRENT_LIST_DIR}/../../runtime ${CMAKE_BINARY_DIR}/hecate-runtime)

add_executable(hello_world
  hello_world.c
)

set_target_properties(hello_world PROPERTIES SUFFIX ".elf")

hecate_runtime_link(hello_world)
