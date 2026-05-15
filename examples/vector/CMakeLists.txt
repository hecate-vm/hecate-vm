cmake_minimum_required(VERSION 3.20)
project(vector CXX)

add_subdirectory(${CMAKE_CURRENT_LIST_DIR}/../../runtime ${CMAKE_BINARY_DIR}/hecate-runtime)

add_executable(vector
  vector.cpp
)

set(ELEMENT_COUNT "1024" CACHE STRING "Number of elements to use")
set(ROUNDS "200" CACHE STRING "Number of iterations to run")

target_compile_definitions(vector PRIVATE
  ELEMENT_COUNT=${ELEMENT_COUNT}
  ROUNDS=${ROUNDS}
)

set_target_properties(vector PROPERTIES SUFFIX ".elf")

hecate_runtime_link(vector)
