#ifndef HECATE_RUNTIME_STDIO_H
#define HECATE_RUNTIME_STDIO_H

#include "hecate_runtime/stddef.h"

/* va_list support (uses compiler builtins, safe under -ffreestanding) */
#ifndef _VA_LIST
#define _VA_LIST
typedef __builtin_va_list va_list;
#endif
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_end(ap) __builtin_va_end(ap)
#define va_arg(ap, type) __builtin_va_arg(ap, type)

#ifdef __cplusplus
extern "C" {
#endif

int putchar(int c);
int puts(const char *s);

/* printf family - supports %d %i %u %x %X %o %b %s %c %p %% */
/* Flags: -, 0, +, space, #. Width and .precision supported.   */
/* Length modifiers: l (long), ll (long long), z (size_t)       */
int printf(const char *fmt, ...);
int snprintf(char *buf, size_t size, const char *fmt, ...);
int vprintf(const char *fmt, va_list ap);
int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);

#ifdef __cplusplus
}
#endif

#endif /* HECATE_RUNTIME_STDIO_H */
