#include "hecate_runtime/stdio.h"
#include "hecate_runtime/string.h"
#include "hecate_runtime/syscalls.h"

int putchar(int c) {
  char ch = (char)c;
  hecate_sys_write(1, &ch, 1);
  return (unsigned char)ch;
}

int puts(const char *s) {
  int len = (int)strlen(s);
  int written = hecate_sys_write(1, s, len);
  hecate_sys_write(1, "\n", 1);
  return written;
}

/* ---------- vsnprintf core ---------- */

/* Write a single character into the output buffer, respecting the limit. */
static void buf_putc(char *buf, size_t size, size_t *pos, char c) {
  if (buf && *pos + 1 < size) {
    buf[*pos] = c;
  }
  (*pos)++;
}

/* Write a string into the buffer. */
static void buf_puts(char *buf, size_t size, size_t *pos, const char *s,
                     int width, int left_align, char pad) {
  int slen = (int)strlen(s);
  int pad_count = (width > slen) ? width - slen : 0;

  if (!left_align) {
    for (int i = 0; i < pad_count; i++) {
      buf_putc(buf, size, pos, pad);
    }
  }
  while (*s) {
    buf_putc(buf, size, pos, *s++);
  }
  if (left_align) {
    for (int i = 0; i < pad_count; i++) {
      buf_putc(buf, size, pos, ' ');
    }
  }
}

/* Compute n / d and n % d without using 64-bit / or %. */
static void u64_divmod_u32(unsigned long long n, unsigned int d,
                           unsigned long long *q_out, unsigned int *r_out) {
  unsigned long long q = 0;
  unsigned int r = 0;

  for (int i = 63; i >= 0; i--) {
    r = (unsigned int)((r << 1) | (unsigned int)((n >> i) & 1ULL));
    if (r >= d) {
      r -= d;
      q |= (1ULL << i);
    }
  }

  *q_out = q;
  *r_out = r;
}

/* Convert unsigned value to string in given base (2-16). */
static int uint_to_str(unsigned long long val, int base, int upper, char *out) {
  static const char digits_lower[] = "0123456789abcdef";
  static const char digits_upper[] = "0123456789ABCDEF";
  const char *digits = upper ? digits_upper : digits_lower;
  char tmp[66]; /* enough for binary 64-bit */
  int len = 0;

  if (val == 0) {
    out[len++] = '0';
  } else {
    while (val) {
      unsigned long long q = 0;
      unsigned int r = 0;
      u64_divmod_u32(val, (unsigned int)base, &q, &r);
      tmp[len++] = digits[r];
      val = q;
    }
    /* reverse */
    for (int i = 0; i < len; i++) {
      out[i] = tmp[len - 1 - i];
    }
  }
  out[len] = '\0';
  return len;
}

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap) {
  size_t pos = 0;

  /* Ensure we can always NUL-terminate */
  if (buf && size == 0) {
    return 0;
  }

  while (*fmt) {
    if (*fmt != '%') {
      buf_putc(buf, size, &pos, *fmt++);
      continue;
    }
    fmt++; /* skip '%' */

    /* Flags */
    int left_align = 0;
    int force_sign = 0;
    int space_sign = 0;
    int alt_form = 0;
    int zero_pad = 0;
    for (;;) {
      if (*fmt == '-') {
        left_align = 1;
        fmt++;
      } else if (*fmt == '+') {
        force_sign = 1;
        fmt++;
      } else if (*fmt == ' ') {
        space_sign = 1;
        fmt++;
      } else if (*fmt == '#') {
        alt_form = 1;
        fmt++;
      } else if (*fmt == '0') {
        zero_pad = 1;
        fmt++;
      } else
        break;
    }
    if (left_align) {
      zero_pad = 0;
    } /* '-' overrides '0' */

    /* Width */
    int width = 0;
    while (*fmt >= '0' && *fmt <= '9') {
      width = width * 10 + (*fmt - '0');
      fmt++;
    }

    /* Precision */
    int precision = -1;
    if (*fmt == '.') {
      fmt++;
      precision = 0;
      while (*fmt >= '0' && *fmt <= '9') {
        precision = precision * 10 + (*fmt - '0');
        fmt++;
      }
    }

    /* Length modifier */
    int is_long = 0;
    int is_longlong = 0;
    if (*fmt == 'l') {
      is_long = 1;
      fmt++;
      if (*fmt == 'l') {
        is_longlong = 1;
        fmt++;
      }
    } else if (*fmt == 'h') {
      fmt++;
      if (*fmt == 'h') {
        fmt++;
      }
    } else if (*fmt == 'z') {
      is_long = 1; /* size_t ~ unsigned long on RV32 */
      fmt++;
    }

    char spec = *fmt++;
    if (spec == '\0') {
      break;
    }

    if (spec == '%') {
      buf_putc(buf, size, &pos, '%');
      continue;
    }

    if (spec == 'c') {
      char ch = (char)(unsigned char)va_arg(ap, int);
      char tmp[2] = {ch, '\0'};
      buf_puts(buf, size, &pos, tmp, width, left_align, ' ');
      continue;
    }

    if (spec == 's') {
      const char *s = va_arg(ap, const char *);
      if (!s) {
        s = "(null)";
      }
      /* Apply precision as max length */
      if (precision >= 0) {
        /* find length up to precision */
        int slen = 0;
        while (s[slen] && slen < precision) {
          slen++;
        }
        /* Write a limited copy via temp - just write char by char */
        int pad_count = (width > slen) ? width - slen : 0;
        if (!left_align) {
          for (int i = 0; i < pad_count; i++) {
            buf_putc(buf, size, &pos, ' ');
          }
        }
        for (int i = 0; i < slen; i++) {
          buf_putc(buf, size, &pos, s[i]);
        }
        if (left_align) {
          for (int i = 0; i < pad_count; i++) {
            buf_putc(buf, size, &pos, ' ');
          }
        }
      } else {
        buf_puts(buf, size, &pos, s, width, left_align, ' ');
      }
      continue;
    }

    /* Numeric specifiers */
    int base = 10;
    int upper = 0;
    int is_signed = 0;
    int is_pointer = 0;

    switch (spec) {
    case 'd':
    case 'i':
      is_signed = 1;
      base = 10;
      break;
    case 'u':
      is_signed = 0;
      base = 10;
      break;
    case 'x':
      is_signed = 0;
      base = 16;
      upper = 0;
      break;
    case 'X':
      is_signed = 0;
      base = 16;
      upper = 1;
      break;
    case 'o':
      is_signed = 0;
      base = 8;
      break;
    case 'b':
      is_signed = 0;
      base = 2;
      break;
    case 'p':
      is_signed = 0;
      base = 16;
      upper = 0;
      is_pointer = 1;
      alt_form = 1;
      is_long = 1;
      break;
    default:
      buf_putc(buf, size, &pos, spec);
      continue;
    }

    /* Read the value */
    unsigned long long uval;
    long long sval = 0;
    if (is_signed) {
      if (is_longlong) {
        sval = va_arg(ap, long long);
      } else if (is_long) {
        sval = (long long)va_arg(ap, long);
      } else {
        sval = (long long)va_arg(ap, int);
      }
      uval = (sval < 0) ? (unsigned long long)(-(sval + 1)) + 1ULL
                        : (unsigned long long)sval;
    } else {
      if (is_longlong) {
        uval = va_arg(ap, unsigned long long);
      } else if (is_long) {
        uval = (unsigned long long)va_arg(ap, unsigned long);
      } else {
        uval = (unsigned long long)va_arg(ap, unsigned int);
      }
      (void)sval;
    }

    /* Build numeric string */
    char numstr[72];
    uint_to_str(uval, base, upper, numstr);
    int numlen = (int)strlen(numstr);

    /* Prefix: sign or '0x' */
    char prefix[3] = {0};
    int preflen = 0;
    if (is_signed) {
      if (sval < 0) {
        prefix[preflen++] = '-';
      } else if (force_sign) {
        prefix[preflen++] = '+';
      } else if (space_sign) {
        prefix[preflen++] = ' ';
      }
    }
    if (alt_form && base == 16 && uval != 0) {
      prefix[preflen++] = '0';
      prefix[preflen++] = upper ? 'X' : 'x';
    } else if (alt_form && base == 8 && numstr[0] != '0') {
      prefix[preflen++] = '0';
    }
    if (is_pointer && uval == 0) {
      /* Print "(nil)" for NULL pointers */
      buf_puts(buf, size, &pos, "(nil)", width, left_align, ' ');
      continue;
    }

    /* Precision for integers: minimum digit count */
    int mindigits = (precision > numlen) ? precision : numlen;

    /* Total printable width */
    int total = preflen + mindigits;
    int pad_total = (width > total) ? width - total : 0;
    char pad_ch = (zero_pad && precision < 0) ? '0' : ' ';

    if (!left_align && pad_ch == ' ') {
      for (int i = 0; i < pad_total; i++) {
        buf_putc(buf, size, &pos, ' ');
      }
    }
    /* prefix */
    for (int i = 0; i < preflen; i++) {
      buf_putc(buf, size, &pos, prefix[i]);
    }
    if (!left_align && pad_ch == '0') {
      for (int i = 0; i < pad_total; i++) {
        buf_putc(buf, size, &pos, '0');
      }
    }
    /* zero-pad to precision */
    for (int i = numlen; i < mindigits; i++) {
      buf_putc(buf, size, &pos, '0');
    }
    /* digits */
    for (int i = 0; i < numlen; i++) {
      buf_putc(buf, size, &pos, numstr[i]);
    }
    if (left_align) {
      for (int i = 0; i < pad_total; i++) {
        buf_putc(buf, size, &pos, ' ');
      }
    }
  }

  /* NUL-terminate */
  if (buf && size > 0) {
    buf[pos < size ? pos : size - 1] = '\0';
  }

  return (int)pos;
}

int vprintf(const char *fmt, va_list ap) {
  char buf[512];
  int n = vsnprintf(buf, sizeof(buf), fmt, ap);
  if (n > 0) {
    int write_len = (n < (int)sizeof(buf) - 1) ? n : (int)sizeof(buf) - 1;
    hecate_sys_write(1, buf, write_len);
  }
  return n;
}

int snprintf(char *buf, size_t size, const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  int n = vsnprintf(buf, size, fmt, ap);
  va_end(ap);
  return n;
}

int printf(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  int n = vprintf(fmt, ap);
  va_end(ap);
  return n;
}
