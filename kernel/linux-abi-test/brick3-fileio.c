#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
int main(void) {
    int fd = open("/bin/linuxtest", O_RDONLY);
    if (fd < 0) { printf("open FAILED rc=%d\n", fd); return 1; }
    unsigned char m[16];
    int n = (int)read(fd, m, sizeof m);
    printf("openat+read OK: fd=%d n=%d magic=%02x '%c%c%c' (expect 7f 'ELF')\n",
           fd, n, m[0], m[1], m[2], m[3]);
    close(fd);
    return 0;
}
