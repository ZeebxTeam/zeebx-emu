#if defined(__linux__)

__attribute__((weak)) char __libc_single_threaded = 0;

__attribute__((weak, visibility("hidden"))) void *__dso_handle = &__dso_handle;

extern void __cxa_finalize(void *) __attribute__((weak));

__attribute__((destructor)) static void __zeebx_libretro_dtor(void) {
    if (__cxa_finalize) {
        __cxa_finalize(&__dso_handle);
    }
}

#endif
