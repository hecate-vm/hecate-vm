cmake_minimum_required(VERSION 3.20)
project(vector_contiguous CXX)

add_subdirectory(${CMAKE_CURRENT_LIST_DIR}/../../runtime ${CMAKE_BINARY_DIR}/hecate-runtime)

add_executable(vector_contiguous
  vector_contiguous.cpp
)

set_target_properties(vector_contiguous PROPERTIES SUFFIX ".elf")

hecate_runtime_link(vector_contiguous)
