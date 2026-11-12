// QtList.app's executable: runs tools/qtlist.py in this process, so the
// pid launch_app starts is the app that shows the window. It loads the
// Python framework at PYLIB and ignores its own arguments, as launch_app
// passes switches meant for a browser. `qt_list_app` builds it with
//
//     clang -DPYLIB=... -DPYPATH=... -DSCRIPT=... qtlaunch.c
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
  (void)argc;
  void *python = dlopen(PYLIB, RTLD_NOW | RTLD_GLOBAL);
  if (!python) {
    fprintf(stderr, "qtlaunch: %s\n", dlerror());
    return 1;
  }
  int (*run)(int, char **) = (int (*)(int, char **))dlsym(python, "Py_BytesMain");
  if (!run) {
    fprintf(stderr, "qtlaunch: no Py_BytesMain\n");
    return 1;
  }
  setenv("PYTHONPATH", PYPATH, 1);
  char *args[] = {argv[0], "-s", SCRIPT, NULL};
  return run(3, args);
}
