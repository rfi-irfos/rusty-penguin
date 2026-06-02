// hello-win: minimal Windows console app for the Rusty Penguin Wine engine.
// Uses WriteFile to stdout (fd 1) — no GUI, no MessageBox.
// Build: x86_64-w64-mingw32-gcc main.c -o hello-win.exe -nostartfiles -lkernel32
#include <windows.h>

void _start(void) {
    HANDLE out = GetStdHandle(STD_OUTPUT_HANDLE);
    const char msg[] = "Hello from Windows on Rusty Penguin!\r\n";
    DWORD written;
    WriteFile(out, msg, sizeof(msg) - 1, &written, NULL);

    const char msg2[] = "Wine subsystem: NtWriteFile -> kernel serial\r\n";
    WriteFile(out, msg2, sizeof(msg2) - 1, &written, NULL);

    ExitProcess(0);
}
