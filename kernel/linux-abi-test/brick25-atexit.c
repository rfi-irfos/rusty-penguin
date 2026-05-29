#include <stdio.h>
#include <stdlib.h>
static void bye(void) {
    printf("atexit handler RAN (pointer-guard demangle works)\n");
    fflush(stdout);
}
int main(void) {
    atexit(bye);
    printf("main running; registered atexit handler\n");
    fflush(stdout);
    return 0;   // triggers glibc __run_exit_handlers
}
