#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
int main(void) {
    /* heap (malloc → brk/mmap) */
    int n = 2000;
    long *a = malloc(n * sizeof(long));
    long sum = 0;
    for (int i = 0; i < n; i++) { a[i] = (long)i * i; sum += a[i]; }
    /* floating point (SSE) + snprintf */
    double s = 0.0;
    for (int i = 1; i <= 100; i++) s += 1.0 / (double)i;
    char buf[128];
    snprintf(buf, sizeof buf, "sum_sq(0..%d)=%ld  harmonic(100)=%.6f  sqrt2=%.6f", n-1, sum, s, sqrt(2.0));
    printf("malloc+FP+snprintf: %s\n", buf);
    /* a bigger allocation to push mmap */
    char *big = malloc(4 * 1024 * 1024);
    memset(big, 0xAB, 4 * 1024 * 1024);
    printf("4MB malloc+memset OK, big[123456]=0x%02x\n", (unsigned char)big[123456]);
    free(big); free(a);
    printf("Linux ABI brick 2 robustness: heap + float + libc string all work.\n");
    return 0;
}
