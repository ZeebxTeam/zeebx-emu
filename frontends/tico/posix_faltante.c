/* O núcleo foi compilado para aarch64-unknown-linux-gnu: a std pede a glibc.
 * O .nro liga newlib + libnx. Este arquivo dá os nomes que a std chama, por
 * cima do que o Horizon tem — ou recusa com ENOSYS o que não dá para imitar.
 *
 * weak: se a libc já tiver a função, fica a dela. */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <malloc.h>
#include <pthread.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#include <switch.h>

#define PROT_EXEC 4
#define MAP_FAILED ((void *)(intptr_t)-1)

#ifndef AT_PAGESZ
#define AT_PAGESZ 6
#endif

#ifndef _SC_PAGESIZE
#define _SC_PAGESIZE 8
#endif
#ifndef _SC_NPROCESSORS_ONLN
#define _SC_NPROCESSORS_ONLN 84
#endif

struct iovec {
	void *iov_base;
	size_t iov_len;
};

/* aarch64 Linux */
#define NR_clock_nanosleep 115
#define NR_sched_getaffinity 123
#define NR_getrandom 278
#define NR_futex 98
#define FUTEX_WAIT 0
#define FUTEX_WAKE 1

struct Cabeca {
	void *bruto;
	size_t pedido;
};

extern int *__errno(void);

__attribute__((weak)) int *__errno_location(void) {
	return __errno();
}

__attribute__((weak)) int posix_memalign(void **memptr, size_t alignment, size_t size) {
	void *p;
	if (!memptr || alignment < sizeof(void *) || (alignment & (alignment - 1)) != 0) {
		return EINVAL;
	}
	p = memalign(alignment, size);
	if (!p) {
		return ENOMEM;
	}
	*memptr = p;
	return 0;
}

__attribute__((weak)) void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset) {
	(void)addr;
	(void)flags;
	(void)fd;
	(void)offset;
	if (length == 0 || (prot & PROT_EXEC)) {
		errno = ENOSYS;
		return MAP_FAILED;
	}
	{
		size_t total = length + sizeof(struct Cabeca) + 4096;
		void *bruto = malloc(total);
		uintptr_t base;
		uintptr_t alinhado;
		struct Cabeca *cab;
		if (!bruto) {
			errno = ENOMEM;
			return MAP_FAILED;
		}
		base = (uintptr_t)bruto + sizeof(struct Cabeca);
		alinhado = (base + 4095u) & ~(uintptr_t)4095u;
		cab = (struct Cabeca *)(alinhado - sizeof(struct Cabeca));
		cab->bruto = bruto;
		cab->pedido = length;
		return (void *)alinhado;
	}
}

__attribute__((weak)) void *mmap64(void *addr, size_t length, int prot, int flags, int fd, off_t offset) {
	return mmap(addr, length, prot, flags, fd, offset);
}

__attribute__((weak)) int munmap(void *addr, size_t length) {
	(void)length;
	if (!addr || addr == MAP_FAILED) {
		return 0;
	}
	{
		struct Cabeca *cab = (struct Cabeca *)((uintptr_t)addr - sizeof(struct Cabeca));
		free(cab->bruto);
		return 0;
	}
}

__attribute__((weak)) int mprotect(void *addr, size_t length, int prot) {
	(void)addr;
	(void)length;
	if (prot & PROT_EXEC) {
		errno = ENOTSUP;
		return -1;
	}
	return 0;
}

__attribute__((weak)) int open64(const char *path, int flags, ...) {
	mode_t mode = 0;
	if (flags & O_CREAT) {
		va_list ap;
		va_start(ap, flags);
		mode = (mode_t)va_arg(ap, int);
		va_end(ap);
	}
	return open(path, flags, mode);
}

__attribute__((weak)) int openat64(int dirfd, const char *path, int flags, ...) {
	mode_t mode = 0;
	if (flags & O_CREAT) {
		va_list ap;
		va_start(ap, flags);
		mode = (mode_t)va_arg(ap, int);
		va_end(ap);
	}
#ifdef O_CLOEXEC
	(void)dirfd;
	/* newlib do Horizon não tem openat; cai no open pelo caminho absoluto. */
	if (path && path[0] == '/') {
		return open(path, flags, mode);
	}
#endif
	(void)dirfd;
	(void)path;
	(void)flags;
	(void)mode;
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) off_t lseek64(int fd, off_t offset, int whence) {
	return lseek(fd, offset, whence);
}

__attribute__((weak)) int fstat64(int fd, struct stat *st) {
	return fstat(fd, st);
}

__attribute__((weak)) int stat64(const char *path, struct stat *st) {
	return stat(path, st);
}

__attribute__((weak)) int lstat64(const char *path, struct stat *st) {
#ifdef lstat
	return lstat(path, st);
#else
	return stat(path, st);
#endif
}

__attribute__((weak)) int fstatat64(int dirfd, const char *path, struct stat *st, int flags) {
	(void)dirfd;
	(void)flags;
	if (path && path[0] == '/') {
		return stat(path, st);
	}
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) int ftruncate64(int fd, off_t length) {
	return ftruncate(fd, length);
}

__attribute__((weak)) struct dirent *readdir64(DIR *dirp) {
	return readdir(dirp);
}

__attribute__((weak)) int dirfd(DIR *dirp) {
	(void)dirp;
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) DIR *fdopendir(int fd) {
	(void)fd;
	errno = ENOSYS;
	return NULL;
}

__attribute__((weak)) int unlinkat(int dirfd, const char *path, int flags) {
	(void)dirfd;
	(void)flags;
	if (path && path[0] == '/') {
		return unlink(path);
	}
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) ssize_t writev(int fd, const struct iovec *iov, int iovcnt) {
	ssize_t total = 0;
	int i;
	if (!iov || iovcnt < 0) {
		errno = EINVAL;
		return -1;
	}
	for (i = 0; i < iovcnt; i++) {
		ssize_t n = write(fd, iov[i].iov_base, iov[i].iov_len);
		if (n < 0) {
			return total > 0 ? total : -1;
		}
		total += n;
		if ((size_t)n < iov[i].iov_len) {
			break;
		}
	}
	return total;
}

__attribute__((weak)) ssize_t sendfile64(int out_fd, int in_fd, off_t *offset, size_t count) {
	(void)out_fd;
	(void)in_fd;
	(void)offset;
	(void)count;
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) ssize_t splice(int fd_in, long long *off_in, int fd_out, long long *off_out,
	size_t len, unsigned int flags) {
	(void)fd_in;
	(void)off_in;
	(void)fd_out;
	(void)off_out;
	(void)len;
	(void)flags;
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) long sysconf(int name) {
	switch (name) {
	case _SC_PAGESIZE:
		return 0x1000;
	case _SC_NPROCESSORS_ONLN:
		return 3; /* três núcleos de aplicação no Horizon */
	default:
		errno = EINVAL;
		return -1;
	}
}

__attribute__((weak)) unsigned long getauxval(unsigned long type) {
	if (type == AT_PAGESZ) {
		return 0x1000;
	}
	return 0;
}

__attribute__((weak)) const char *gnu_get_libc_version(void) {
	return "2.38";
}

__attribute__((weak)) uid_t geteuid(void) {
	return 0;
}

__attribute__((weak)) int pause(void) {
	svcSleepThread(1000000000ULL);
	errno = EINTR;
	return -1;
}

__attribute__((weak)) int clock_nanosleep(clockid_t clock_id, int flags, const struct timespec *req,
	struct timespec *rem) {
	(void)clock_id;
	(void)flags;
	if (!req) {
		errno = EFAULT;
		return -1;
	}
	{
		u64 ns = (u64)req->tv_sec * 1000000000ULL + (u64)req->tv_nsec;
		if (ns == 0) {
			ns = 1;
		}
		svcSleepThread(ns);
		if (rem) {
			rem->tv_sec = 0;
			rem->tv_nsec = 0;
		}
		return 0;
	}
}

__attribute__((weak)) int sigaltstack(const stack_t *ss, stack_t *old_ss) {
	(void)ss;
	if (old_ss) {
		memset(old_ss, 0, sizeof(*old_ss));
	}
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) int sched_getaffinity(int pid, size_t cpusetsize, void *mask) {
	(void)pid;
	if (!mask || cpusetsize == 0) {
		errno = EINVAL;
		return -1;
	}
	memset(mask, 0, cpusetsize);
	/* Três núcleos: bits 0..2. */
	((unsigned char *)mask)[0] = 0x7;
	return 0;
}

__attribute__((weak)) int pthread_setname_np(pthread_t thread, const char *name) {
	(void)thread;
	(void)name;
	return 0;
}

__attribute__((weak)) int fchown(int fd, uid_t owner, gid_t group) {
	(void)fd;
	(void)owner;
	(void)group;
	errno = ENOSYS;
	return -1;
}

__attribute__((weak)) int __res_init(void) {
	return 0;
}

__attribute__((weak)) void *dlsym(void *handle, const char *symbol) {
	(void)handle;
	(void)symbol;
	return NULL;
}

__attribute__((weak)) int dl_iterate_phdr(int (*callback)(void *info, size_t size, void *data), void *data) {
	(void)callback;
	(void)data;
	return 0;
}

/* Futex mínimo: WAIT dorme um pouco e reolha; WAKE não faz nada além de
 * voltar. Serve para a std não abortar; não é um futex de verdade. */
static int futex_minimo(uint32_t *uaddr, int op, uint32_t val) {
	if (!uaddr) {
		errno = EFAULT;
		return -1;
	}
	op &= 0x7f; /* ignora PRIVATE/CLOCK */
	if (op == FUTEX_WAIT) {
		if (__atomic_load_n(uaddr, __ATOMIC_SEQ_CST) != val) {
			errno = EAGAIN;
			return -1;
		}
		svcSleepThread(1000000ULL); /* 1 ms */
		return 0;
	}
	if (op == FUTEX_WAKE) {
		return 0;
	}
	errno = ENOSYS;
	return -1;
}

static long getrandom_libnx(void *buf, size_t buflen, unsigned int flags) {
	(void)flags;
	if (!buf) {
		errno = EFAULT;
		return -1;
	}
	if (R_SUCCEEDED(csrngInitialize())) {
		Result rc = csrngGetRandomBytes(buf, buflen);
		csrngExit();
		if (R_SUCCEEDED(rc)) {
			return (long)buflen;
		}
	}
	/* Sem csrng, preenche com o tick — melhor que falhar a std na largada. */
	{
		size_t i;
		u64 t = armGetSystemTick();
		for (i = 0; i < buflen; i++) {
			t = t * 6364136223846793005ULL + 1;
			((unsigned char *)buf)[i] = (unsigned char)(t >> 33);
		}
	}
	return (long)buflen;
}

__attribute__((weak)) long syscall(long number, ...) {
	va_list ap;
	va_start(ap, number);
	switch (number) {
	case NR_getrandom: {
		void *buf = va_arg(ap, void *);
		size_t len = va_arg(ap, size_t);
		unsigned int flags = va_arg(ap, unsigned int);
		va_end(ap);
		return getrandom_libnx(buf, len, flags);
	}
	case NR_futex: {
		uint32_t *uaddr = va_arg(ap, uint32_t *);
		int op = va_arg(ap, int);
		uint32_t val = va_arg(ap, uint32_t);
		va_end(ap);
		return futex_minimo(uaddr, op, val);
	}
	case NR_clock_nanosleep: {
		clockid_t clock_id = va_arg(ap, clockid_t);
		int flags = va_arg(ap, int);
		const struct timespec *req = va_arg(ap, const struct timespec *);
		struct timespec *rem = va_arg(ap, struct timespec *);
		va_end(ap);
		return clock_nanosleep(clock_id, flags, req, rem);
	}
	case NR_sched_getaffinity: {
		int pid = va_arg(ap, int);
		size_t size = va_arg(ap, size_t);
		void *mask = va_arg(ap, void *);
		va_end(ap);
		return sched_getaffinity(pid, size, mask);
	}
	default:
		va_end(ap);
		errno = ENOSYS;
		return -1;
	}
}
