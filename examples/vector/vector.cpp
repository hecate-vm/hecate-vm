/*
 * Data-oriented benchmark: contiguous vector scan.
 *
 * Same logical work as linked_list: sum N values for STEPS rounds.
 * The difference is memory access pattern: this is linear and contiguous.
 */

#include "hecate_runtime/syscalls.h"
#include "hecate_runtime/stdlib.h"

#ifndef ELEMENT_COUNT
#define ELEMENT_COUNT 1024
#endif

#ifndef ROUNDS
#define ROUNDS 200
#endif

template <typename T> struct Vector {
  T *data;
  size_t size;
  size_t capacity;
  Vector() : data(nullptr), size(0), capacity(0) {}
  T &operator[](size_t i) const { return data[i]; }
  void push_back(const T &value) {
    if (size == capacity) {
      size_t new_capacity = capacity == 0 ? 4 : capacity * 2;
      T *new_data = static_cast<T *>(malloc(new_capacity * sizeof(T)));
      if (!new_data) {
        hecate_sys_exit(1);
      }
      for (size_t i = 0; i < size; i++) {
        new_data[i] = data[i];
      }
      free(data);
      data = new_data;
      capacity = new_capacity;
    }
    data[size++] = value;
  }
  ~Vector() { free(data); }
};

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
  Vector<int> values;
  for (int i = 0; i < ELEMENT_COUNT; i++) {
    values.push_back((i * 37) + 11);
  }

  int acc = 0;
  for (int step = 0; step < ROUNDS; step++) {
    for (int i = 0; i < ELEMENT_COUNT; i++) {
      acc += values[i];
    }
  }

  print_int(acc);
  return 0;
}