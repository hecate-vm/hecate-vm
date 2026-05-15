/*
 * Naive benchmark: pointer chasing through a randomized linked list.
 *
 * Nodes are heap-allocated with malloc and linked with pointer fields to
 * emphasize cache-unfriendly pointer chasing.
 */

#include "hecate_runtime/stdlib.h"
#include "hecate_runtime/syscalls.h"

#ifndef ELEMENT_COUNT
#define ELEMENT_COUNT 1024
#endif

#ifndef ROUNDS
#define ROUNDS 200
#endif

struct Node {
  int value;
  Node *next;
};

static unsigned int rng_state = 1u;

static unsigned int next_rand() {
  rng_state = rng_state * 1664525u + 1013904223u;
  return rng_state;
}

static char pbuf[16];

static void print_int(int v) {
  int neg = 0;
  if (v < 0) {
    neg = 1;
    v = -v;
  }
  int i = 15;
  pbuf[i] = '\n';
  if (v == 0) {
    pbuf[--i] = '0';
  } else {
    while (v > 0) {
      pbuf[--i] = (char)('0' + (v % 10));
      v /= 10;
    }
  }
  if (neg) {
    pbuf[--i] = '-';
  }
  hecate_sys_write(1, pbuf + i, 16 - i);
}

int main() {
  Node *head = nullptr;

  for (int i = 0; i < ELEMENT_COUNT; i++) {
    Node *n = static_cast<Node *>(malloc(sizeof(Node)));
    if (!n) {
      hecate_sys_exit(1);
    }
    n->value = (i * 37) + 11;
    n->next = nullptr;

    if (!head || (next_rand() & 1u) == 0u) {
      n->next = head;
      head = n;
    } else {
      Node *cur = head;
      unsigned int hops = next_rand() & 63u;
      while (hops != 0u && cur->next) {
        cur = cur->next;
        hops--;
      }
      n->next = cur->next;
      cur->next = n;
    }
  }

  int acc = 0;
  for (int step = 0; step < ROUNDS; step++) {
    Node *cur = head;
    while (cur) {
      acc += cur->value;
      cur = cur->next;
    }
  }

  Node *cur = head;
  while (cur) {
    Node *next = cur->next;
    free(cur);
    cur = next;
  }

  print_int(acc);
  return 0;
}