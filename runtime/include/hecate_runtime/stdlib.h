#ifndef HECATE_RUNTIME_STDLIB_H
#define HECATE_RUNTIME_STDLIB_H

typedef __SIZE_TYPE__ size_t;

#ifdef __cplusplus
extern "C" {
#endif

void *malloc(size_t size);
void free(void *ptr);

#ifdef __cplusplus
}
#endif

#endif