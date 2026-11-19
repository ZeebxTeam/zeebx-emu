/* fchown and readlink exist in the Vita newlib header but not in the libc.
 * sqlite stores the address of both in its syscall table. The RetroArch
 * process is not root, so fchown never actually gets called; a readlink
 * that fails is what sqlite treats as "this path is not a symlink". */
#include <errno.h>
#include <stddef.h>
#include <sys/types.h>
#include <unistd.h>

int fchown(int fd, uid_t owner, gid_t group) {
	(void)fd;
	(void)owner;
	(void)group;
	errno = ENOSYS;
	return -1;
}

ssize_t readlink(const char *path, char *buf, size_t bufsiz) {
	(void)path;
	(void)buf;
	(void)bufsiz;
	errno = EINVAL;
	return -1;
}
