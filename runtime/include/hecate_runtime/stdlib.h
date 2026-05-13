#ifndef HECATE_RUNTIME_STDLIB_H
#define HECATE_RUNTIME_STDLIB_H

#include "hecate_runtime/stddef.h"

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

#ifdef __cplusplus
extern "C" {
#endif

/* Memory allocation */
void *malloc(size_t size);
void *calloc(size_t nmemb, size_t size);
void *realloc(void *ptr, size_t size);
void free(void *ptr);

/* Program control */
void abort(void) __attribute__((noreturn));
void exit(int status) __attribute__((noreturn));

/* Integer math */
int abs(int n);
long labs(long n);

/* String conversion */
int atoi(const char *s);
long atol(const char *s);
char *itoa(int value, char *buf, int base);

#ifdef __cplusplus
}
#endif

#endif /* HECATE_RUNTIME_STDLIB_H */