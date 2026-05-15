cmake_minimum_required(VERSION 3.20)
project(linked_list CXX)

add_subdirectory(${CMAKE_CURRENT_LIST_DIR}/../../runtime ${CMAKE_BINARY_DIR}/hecate-runtime)

add_executable(linked_list
  linked_list.cpp
)

set(LINKED_LIST_NODE_COUNT "1024" CACHE STRING "Number of linked-list nodes to allocate")
set(LINKED_LIST_TRAVERSAL_ROUNDS "200" CACHE STRING "Number of full-list traversal rounds")

target_compile_definitions(linked_list PRIVATE
  LINKED_LIST_NODE_COUNT=${LINKED_LIST_NODE_COUNT}
  LINKED_LIST_TRAVERSAL_ROUNDS=${LINKED_LIST_TRAVERSAL_ROUNDS}
)

set_target_properties(linked_list PROPERTIES SUFFIX ".elf")

hecate_runtime_link(linked_list)
