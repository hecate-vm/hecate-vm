#include "hecate_runtime/syscalls.h"

int hecate_sys_write(int fd, const char *buf, int len) {
  register int a0 __asm__("a0") = fd;
  register const char *a1 __asm__("a1") = buf;
  register int a2 __asm__("a2") = len;
  register int a7 __asm__("a7") = HECATE_SYS_WRITE;
  __asm__ volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
  return a0;
}

__attribute__((noreturn)) void hecate_sys_exit(int code) {
  register int a0 __asm__("a0") = code;
  register int a7 __asm__("a7") = HECATE_SYS_EXIT;
  __asm__ volatile("ecall" : : "r"(a0), "r"(a7) : "memory");
  for (;;) {
  }
}
