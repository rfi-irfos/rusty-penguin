#include <stdio.h>
#include <unistd.h>
__thread int tls_val = 0x1234;
int main(void) {
    printf("glibc printf works. tls_val=0x%x (expect 0x1234)\n", tls_val);
    tls_val = 0x5678;
    printf("TLS write/read: tls_val=0x%x (expect 0x5678)\n", tls_val);
    fflush(stdout);
    _exit(0);
}
