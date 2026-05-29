/* A freestanding static Linux x86-64 ELF: no libc, raw syscalls only.
   Brick 1 of the Linux-ABI layer — proves the pure-Rust kernel can execute
   an unmodified Linux binary via the real syscall convention. */
static long sys_write(int fd, const void *buf, unsigned long n) {
    long r;
    __asm__ volatile("syscall" : "=a"(r)
        : "a"(1), "D"(fd), "S"(buf), "d"(n) : "rcx", "r11", "memory");
    return r;
}
static __attribute__((noreturn)) void sys_exit_group(int code) {
    __asm__ volatile("syscall" : : "a"(231), "D"(code) : "rcx", "r11", "memory");
    for (;;) {}
}
void _start(void) {
    const char msg[] =
        "Hello from a REAL unmodified Linux x86-64 ELF,\n"
        "running on the from-scratch pure-Rust Rusty Penguin kernel.\n"
        "Linux ABI brick 1: write(2) + exit_group(2) over the syscall ABI.\n";
    sys_write(1, msg, sizeof(msg) - 1);
    sys_exit_group(0);
}
