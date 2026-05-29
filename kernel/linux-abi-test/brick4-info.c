#include <stdio.h>
#include <unistd.h>
#include <sys/utsname.h>
int main(void){
  struct utsname u; uname(&u);
  printf("uname: %s %s %s [%s]\n", u.sysname, u.nodename, u.release, u.machine);
  char cwd[64]=""; getcwd(cwd,sizeof cwd);
  printf("pid=%d uid=%d cwd=%s\n", (int)getpid(), (int)getuid(), cwd);
  return 0;
}
