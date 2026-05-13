#ifndef HECATE_RUNTIME_SYSCALLS_H
#define HECATE_RUNTIME_SYSCALLS_H

#define HECATE_SYS_WRITE 64
#define HECATE_SYS_EXIT 93

int hecate_sys_write(int fd, const char *buf, int len);
__attribute__((noreturn)) void hecate_sys_exit(int code);

#endif
