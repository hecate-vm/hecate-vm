cmake_minimum_required(VERSION 3.20)
project(linked_list CXX)

add_subdirectory(${CMAKE_CURRENT_LIST_DIR}/../../runtime ${CMAKE_BINARY_DIR}/hecate-runtime)

add_executable(linked_list
  linked_list.cpp
)

set_target_properties(linked_list PROPERTIES SUFFIX ".elf")

hecate_runtime_link(linked_list)
