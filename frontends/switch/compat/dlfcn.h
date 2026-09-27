/* devkitA64 newlib has no dlfcn.h. SQLite includes it when load_extension is on. */
#ifndef _DLFCN_H
#define _DLFCN_H

#define RTLD_LAZY 1
#define RTLD_NOW 2
#define RTLD_GLOBAL 256
#define RTLD_LOCAL 0

#ifdef __cplusplus
extern "C" {
#endif

void *dlopen(const char *filename, int flags);
void *dlsym(void *handle, const char *symbol);
int dlclose(void *handle);
char *dlerror(void);

#ifdef __cplusplus
}
#endif

#endif
