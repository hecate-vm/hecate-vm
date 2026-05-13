#include "hecate_runtime/syscalls.h"

extern int main(void) __attribute__((weak));
extern int hecate_cpp_main_void(void) __asm__("_Z4mainv") __attribute__((weak));
extern int hecate_cpp_main_argc_argv(int, char **) __asm__("_Z4mainiPPc")
    __attribute__((weak));

__attribute__((noreturn)) void _start(void) {
  int code = 1;

  if (main) {
    code = main();
  } else if (hecate_cpp_main_void) {
    code = hecate_cpp_main_void();
  } else if (hecate_cpp_main_argc_argv) {
    code = hecate_cpp_main_argc_argv(0, (char **)0);
  }

  hecate_sys_exit(code);
}
