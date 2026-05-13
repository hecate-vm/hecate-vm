function(hecate_runtime_apply_rv32_compile_options target_name)
  target_compile_options(${target_name} PRIVATE
    -ffreestanding
    -fno-builtin
    -march=rv32im
    -mabi=ilp32
  )
endfunction()

function(hecate_runtime_apply_rv32_link_options target_name)
  target_link_options(${target_name} PRIVATE
    -nostdlib
    -Wl,-e,_start
    -Wl,-Ttext=0x10000000
  )
endfunction()

function(hecate_runtime_link target_name)
  if(NOT TARGET hecate_runtime_rv32)
    message(FATAL_ERROR "hecate_runtime_rv32 target is missing. Add runtime/ with add_subdirectory(...) first.")
  endif()

  target_link_libraries(${target_name} PRIVATE
    hecate_runtime_headers
    hecate_runtime_rv32
  )

  hecate_runtime_apply_rv32_compile_options(${target_name})
  hecate_runtime_apply_rv32_link_options(${target_name})
endfunction()
