#ifndef HECATE_RUNTIME_STDINT_H
#define HECATE_RUNTIME_STDINT_H

typedef signed char        int8_t;
typedef signed short       int16_t;
typedef signed int         int32_t;
typedef signed long long   int64_t;

typedef unsigned char      uint8_t;
typedef unsigned short     uint16_t;
typedef unsigned int       uint32_t;
typedef unsigned long long uint64_t;

/* Pointer-sized types (RV32: 32-bit) */
typedef int32_t  intptr_t;
typedef uint32_t uintptr_t;
typedef int32_t  ptrdiff_t;

typedef int64_t  intmax_t;
typedef uint64_t uintmax_t;

/* Limits */
#define INT8_MIN    (-128)
#define INT8_MAX    (127)
#define INT16_MIN   (-32768)
#define INT16_MAX   (32767)
#define INT32_MIN   (-2147483647 - 1)
#define INT32_MAX   (2147483647)
#define INT64_MIN   (-9223372036854775807LL - 1)
#define INT64_MAX   (9223372036854775807LL)

#define UINT8_MAX   (255u)
#define UINT16_MAX  (65535u)
#define UINT32_MAX  (4294967295u)
#define UINT64_MAX  (18446744073709551615ULL)

#define INTPTR_MIN  INT32_MIN
#define INTPTR_MAX  INT32_MAX
#define UINTPTR_MAX UINT32_MAX
#define SIZE_MAX    UINT32_MAX

#define INT8_C(x)   (x)
#define INT16_C(x)  (x)
#define INT32_C(x)  (x)
#define INT64_C(x)  (x ## LL)

#define UINT8_C(x)  (x ## u)
#define UINT16_C(x) (x ## u)
#define UINT32_C(x) (x ## u)
#define UINT64_C(x) (x ## ULL)

#endif /* HECATE_RUNTIME_STDINT_H */
