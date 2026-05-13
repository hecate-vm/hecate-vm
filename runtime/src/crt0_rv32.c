#include "hecate_runtime/syscalls.h"

extern int main(void);

__attribute__((noreturn)) void _start(void) {
  int code = main();
  hecate_sys_exit(code);
}
