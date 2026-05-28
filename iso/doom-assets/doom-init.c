/* Minimal PID 1 for the Rusty Penguin DOOM live image.
 * Mounts the pseudo-filesystems DOOM needs (/dev for /dev/fb0 and the
 * tty, /proc, /sys), then hands control to fbdoom on the bundled WAD.
 * Built static so it carries no library dependencies of its own. */
#include <sys/mount.h>
#include <sys/wait.h>
#include <unistd.h>
#include <stdio.h>

int main(void) {
    fputs("\nRusty Penguin -- DOOM live image\n"
          "Binary hardware. Ternary mind. Now with chainguns.\n\n", stdout);
    fflush(stdout);

    mount("proc",  "/proc", "proc",     0, "");
    mount("sysfs", "/sys",  "sysfs",    0, "");
    /* devtmpfs gives us /dev/fb0 and /dev/tty without manual mknod */
    mount("dev",   "/dev",  "devtmpfs", 0, "");

    char *argv[] = { "/fbdoom", "-iwad", "/doom1.wad", 0 };
    char *envp[] = { "HOME=/", "TERM=linux", 0 };

    pid_t pid = fork();
    if (pid == 0) {
        execve("/fbdoom", argv, envp);
        perror("execve fbdoom");
        _exit(127);
    }
    int st;
    waitpid(pid, &st, 0);

    /* DOOM exited (or never started). Keep PID 1 alive so the kernel
     * does not panic; the user can power off the VM. */
    for (;;) pause();
    return 0;
}
