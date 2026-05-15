cmake_minimum_required(VERSION 3.20)
project(vector_contiguous CXX)

add_subdirectory(${CMAKE_CURRENT_LIST_DIR}/../../runtime ${CMAKE_BINARY_DIR}/hecate-runtime)

add_executable(vector_contiguous
  vector_contiguous.cpp
)

set(VECTOR_CONTIGUOUS_ELEMENT_COUNT "1024" CACHE STRING "Number of elements to populate in the vector benchmark")
set(VECTOR_CONTIGUOUS_SCAN_ROUNDS "200" CACHE STRING "Number of repeated full-vector scans")

target_compile_definitions(vector_contiguous PRIVATE
  VECTOR_CONTIGUOUS_ELEMENT_COUNT=${VECTOR_CONTIGUOUS_ELEMENT_COUNT}
  VECTOR_CONTIGUOUS_SCAN_ROUNDS=${VECTOR_CONTIGUOUS_SCAN_ROUNDS}
)

set_target_properties(vector_contiguous PROPERTIES SUFFIX ".elf")

hecate_runtime_link(vector_contiguous)
