#include "hecate_runtime/string.h"

/* ---------- memory ---------- */

void *memcpy(void *dest, const void *src, size_t n) {
  unsigned char *d = (unsigned char *)dest;
  const unsigned char *s = (const unsigned char *)src;
  for (size_t i = 0; i < n; i++) {
    d[i] = s[i];
  }
  return dest;
}

void *memmove(void *dest, const void *src, size_t n) {
  unsigned char *d = (unsigned char *)dest;
  const unsigned char *s = (const unsigned char *)src;
  if (d < s || d >= s + n) {
    for (size_t i = 0; i < n; i++) {
      d[i] = s[i];
    }
  } else {
    /* Copy backwards to handle overlap */
    size_t i = n;
    while (i--) {
      d[i] = s[i];
    }
  }
  return dest;
}

void *memset(void *s, int c, size_t n) {
  unsigned char *p = (unsigned char *)s;
  unsigned char b = (unsigned char)c;
  for (size_t i = 0; i < n; i++) {
    p[i] = b;
  }
  return s;
}

int memcmp(const void *s1, const void *s2, size_t n) {
  const unsigned char *a = (const unsigned char *)s1;
  const unsigned char *b = (const unsigned char *)s2;
  for (size_t i = 0; i < n; i++) {
    if (a[i] != b[i]) {
      return (int)a[i] - (int)b[i];
    }
  }
  return 0;
}

void *memchr(const void *s, int c, size_t n) {
  const unsigned char *p = (const unsigned char *)s;
  unsigned char b = (unsigned char)c;
  for (size_t i = 0; i < n; i++) {
    if (p[i] == b) {
      return (void *)(p + i);
    }
  }
  return (void *)0;
}

/* ---------- string ---------- */

size_t strlen(const char *s) {
  size_t n = 0;
  while (s[n] != '\0') {
    n++;
  }
  return n;
}

char *strcpy(char *dest, const char *src) {
  char *d = dest;
  while ((*d++ = *src++) != '\0') {
  }
  return dest;
}

char *strncpy(char *dest, const char *src, size_t n) {
  size_t i;
  for (i = 0; i < n && src[i] != '\0'; i++) {
    dest[i] = src[i];
  }
  for (; i < n; i++) {
    dest[i] = '\0';
  }
  return dest;
}

char *strcat(char *dest, const char *src) {
  char *d = dest + strlen(dest);
  while ((*d++ = *src++) != '\0') {
  }
  return dest;
}

char *strncat(char *dest, const char *src, size_t n) {
  char *d = dest + strlen(dest);
  size_t i;
  for (i = 0; i < n && src[i] != '\0'; i++) {
    d[i] = src[i];
  }
  d[i] = '\0';
  return dest;
}

int strcmp(const char *s1, const char *s2) {
  while (*s1 && *s1 == *s2) {
    s1++;
    s2++;
  }
  return (unsigned char)*s1 - (unsigned char)*s2;
}

int strncmp(const char *s1, const char *s2, size_t n) {
  for (size_t i = 0; i < n; i++) {
    unsigned char a = (unsigned char)s1[i];
    unsigned char b = (unsigned char)s2[i];
    if (a != b) {
      return (int)a - (int)b;
    }
    if (a == '\0') {
      break;
    }
  }
  return 0;
}

char *strchr(const char *s, int c) {
  char ch = (char)c;
  while (*s != '\0') {
    if (*s == ch) {
      return (char *)s;
    }
    s++;
  }
  return (ch == '\0') ? (char *)s : (char *)0;
}

char *strrchr(const char *s, int c) {
  char ch = (char)c;
  const char *last = (char *)0;
  while (*s != '\0') {
    if (*s == ch) {
      last = s;
    }
    s++;
  }
  if (ch == '\0') {
    return (char *)s;
  }
  return (char *)last;
}

char *strstr(const char *haystack, const char *needle) {
  if (*needle == '\0') {
    return (char *)haystack;
  }
  size_t nlen = strlen(needle);
  size_t hlen = strlen(haystack);
  if (nlen > hlen) {
    return (char *)0;
  }
  for (size_t i = 0; i <= hlen - nlen; i++) {
    if (memcmp(haystack + i, needle, nlen) == 0) {
      return (char *)(haystack + i);
    }
  }
  return (char *)0;
}

size_t strspn(const char *s, const char *accept) {
  size_t n = 0;
  while (s[n] != '\0') {
    if (!strchr(accept, (unsigned char)s[n])) {
      break;
    }
    n++;
  }
  return n;
}

size_t strcspn(const char *s, const char *reject) {
  size_t n = 0;
  while (s[n] != '\0') {
    if (strchr(reject, (unsigned char)s[n])) {
      break;
    }
    n++;
  }
  return n;
}

char *strpbrk(const char *s, const char *accept) {
  while (*s != '\0') {
    if (strchr(accept, (unsigned char)*s)) {
      return (char *)s;
    }
    s++;
  }
  return (char *)0;
}