/* Brick-3 test: a Linux process opens /dev/fb0, mmaps it, fills it with orange,
   and reports. Run scheduled, the kernel hands it a PRIVATE 640x400 surface
   (not the real hardware framebuffer) and reads it back. */
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>
#include <stdint.h>
int main(void) {
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) { write(1, "NOFB\n", 5); return 1; }
    long n = 640L * 400L;
    uint32_t *fb = mmap(0, n * 4, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (fb == MAP_FAILED) { write(1, "NOMMAP\n", 7); return 1; }
    for (long i = 0; i < n; i++) fb[i] = 0x00FF8800; /* orange */
    write(1, "FBFILLED\n", 9);
    return 0;
}
