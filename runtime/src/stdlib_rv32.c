#include "hecate_runtime/stdlib.h"
#include "hecate_runtime/string.h"
#include "hecate_runtime/syscalls.h"

/* ---------- heap allocator (first-fit, 256 KB static pool) ---------- */

#define HECATE_HEAP_SIZE (256u * 1024u)
#define HECATE_ALIGN 8u

typedef struct BlockHeader {
  size_t size;
  int is_free;
  struct BlockHeader *next;
} BlockHeader;

static unsigned char hecate_heap[HECATE_HEAP_SIZE]
    __attribute__((aligned(HECATE_ALIGN)));
static BlockHeader *heap_head = (BlockHeader *)0;

static size_t align_up(size_t n) {
  return (n + (HECATE_ALIGN - 1u)) & ~(HECATE_ALIGN - 1u);
}

static void heap_init(void) {
  if (heap_head) {
    return;
  }

  heap_head = (BlockHeader *)(void *)hecate_heap;
  heap_head->size = HECATE_HEAP_SIZE - sizeof(BlockHeader);
  heap_head->is_free = 1;
  heap_head->next = (BlockHeader *)0;
}

static void split_block(BlockHeader *block, size_t wanted) {
  if (block->size <= wanted + sizeof(BlockHeader) + HECATE_ALIGN) {
    return;
  }

  unsigned char *new_addr = (unsigned char *)(void *)(block + 1) + wanted;
  BlockHeader *tail = (BlockHeader *)(void *)new_addr;
  tail->size = block->size - wanted - sizeof(BlockHeader);
  tail->is_free = 1;
  tail->next = block->next;

  block->size = wanted;
  block->next = tail;
}

static void coalesce_free_blocks(void) {
  BlockHeader *cur = heap_head;
  while (cur && cur->next) {
    BlockHeader *next = cur->next;
    unsigned char *expected = (unsigned char *)(void *)(cur + 1) + cur->size;
    if (cur->is_free && next->is_free &&
        (unsigned char *)(void *)next == expected) {
      cur->size += sizeof(BlockHeader) + next->size;
      cur->next = next->next;
      continue;
    }
    cur = cur->next;
  }
}

void *malloc(size_t size) {
  if (size == 0) {
    return (void *)0;
  }

  heap_init();
  size_t wanted = align_up(size);

  BlockHeader *cur = heap_head;
  while (cur) {
    if (cur->is_free && cur->size >= wanted) {
      split_block(cur, wanted);
      cur->is_free = 0;
      return (void *)(cur + 1);
    }
    cur = cur->next;
  }

  return (void *)0;
}

void free(void *ptr) {
  if (!ptr) {
    return;
  }

  BlockHeader *block = ((BlockHeader *)ptr) - 1;
  block->is_free = 1;
  coalesce_free_blocks();
}

void *calloc(size_t nmemb, size_t size) {
  /* Check for multiplication overflow */
  if (size != 0 && nmemb > HECATE_HEAP_SIZE / size) {
    return (void *)0;
  }
  size_t total = nmemb * size;
  void *ptr = malloc(total);
  if (ptr) {
    memset(ptr, 0, total);
  }
  return ptr;
}

void *realloc(void *ptr, size_t size) {
  if (!ptr) {
    return malloc(size);
  }
  if (size == 0) {
    free(ptr);
    return (void *)0;
  }

  BlockHeader *block = ((BlockHeader *)ptr) - 1;
  size_t old_size = block->size;

  if (align_up(size) <= old_size) {
    /* Block is already large enough - split if possible */
    split_block(block, align_up(size));
    return ptr;
  }

  /* Allocate new block, copy, free old */
  void *new_ptr = malloc(size);
  if (!new_ptr) {
    return (void *)0;
  }
  memcpy(new_ptr, ptr, old_size < size ? old_size : size);
  free(ptr);
  return new_ptr;
}

/* ---------- program control ---------- */

void abort(void) {
  hecate_sys_exit(134); /* SIGABRT conventional exit code */
  __builtin_unreachable();
}

void exit(int status) {
  hecate_sys_exit(status);
  __builtin_unreachable();
}

/* ---------- integer math ---------- */

int abs(int n) { return (n < 0) ? -n : n; }

long labs(long n) { return (n < 0) ? -n : n; }

/* ---------- string conversion ---------- */

int atoi(const char *s) { return (int)atol(s); }

long atol(const char *s) {
  /* Skip leading whitespace */
  while (*s == ' ' || *s == '\t' || *s == '\n' || *s == '\r' || *s == '\f' ||
         *s == '\v') {
    s++;
  }

  int neg = 0;
  if (*s == '-') {
    neg = 1;
    s++;
  } else if (*s == '+') {
    s++;
  }

  long result = 0;
  while (*s >= '0' && *s <= '9') {
    result = result * 10 + (*s - '0');
    s++;
  }

  return neg ? -result : result;
}

char *itoa(int value, char *buf, int base) {
  if (base < 2 || base > 36) {
    buf[0] = '\0';
    return buf;
  }

  static const char digits[] = "0123456789abcdefghijklmnopqrstuvwxyz";
  char tmp[34];
  int len = 0;
  int neg = 0;

  unsigned int uval;
  if (value < 0 && base == 10) {
    neg = 1;
    uval = (unsigned int)(-(value + 1)) + 1u;
  } else {
    uval = (unsigned int)value;
  }

  if (uval == 0) {
    tmp[len++] = '0';
  } else {
    while (uval) {
      tmp[len++] = digits[uval % (unsigned)base];
      uval /= (unsigned)base;
    }
  }

  if (neg) {
    tmp[len++] = '-';
  }

  /* reverse into buf */
  for (int i = 0; i < len; i++) {
    buf[i] = tmp[len - 1 - i];
  }
  buf[len] = '\0';
  return buf;
}