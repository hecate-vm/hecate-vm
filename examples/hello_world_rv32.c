static inline int syscall3(int n, int a0_in, int a1_in, int a2_in) {
  register int a0 __asm__("a0") = a0_in;
  register int a1 __asm__("a1") = a1_in;
  register int a2 __asm__("a2") = a2_in;
  register int a7 __asm__("a7") = n;
  __asm__ volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
  return a0;
}

static inline __attribute__((noreturn)) void syscall1_noreturn(int n, int a0_in) {
  register int a0 __asm__("a0") = a0_in;
  register int a7 __asm__("a7") = n;
  __asm__ volatile("ecall" : : "r"(a0), "r"(a7) : "memory");
  for (;;) {
  }
}

__attribute__((noreturn)) void _start(void) {
  static const char msg[] = "Hello, world from RV32 C!\n";
  (void)syscall3(64, 1, (int)msg, (int)(sizeof(msg) - 1));
  syscall1_noreturn(93, 0);
}
