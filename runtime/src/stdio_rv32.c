#include "hecate_runtime/stdio.h"
#include "hecate_runtime/syscalls.h"

int puts(const char *s) {
  int len = 0;
  while (s[len] != '\0') {
    len++;
  }

  int written = hecate_sys_write(1, s, len);
  (void)hecate_sys_write(1, "\n", 1);
  return written;
}
