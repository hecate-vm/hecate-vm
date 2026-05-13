#ifndef HECATE_RUNTIME_ASSERT_H
#define HECATE_RUNTIME_ASSERT_H

#include "hecate_runtime/syscalls.h"
#include "hecate_runtime/stdio.h"

#ifdef NDEBUG
#define assert(expr) ((void)0)
#else
/* Stringify helper macros */
#define HECATE_STR_(x) #x
#define HECATE_STR(x)  HECATE_STR_(x)

#define assert(expr)                                                          \
  do {                                                                        \
    if (!(expr)) {                                                            \
      puts("assertion failed: " #expr                                         \
           " (" __FILE__ ":" HECATE_STR(__LINE__) ")");                       \
      hecate_sys_exit(1);                                                     \
    }                                                                         \
  } while (0)
#endif

#endif /* HECATE_RUNTIME_ASSERT_H */
