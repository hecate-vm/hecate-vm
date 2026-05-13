#include "hecate_runtime/stdlib.h"

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