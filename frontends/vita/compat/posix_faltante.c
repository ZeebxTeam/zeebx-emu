/* fchown e readlink existem no cabeçalho da newlib da Vita e não na libc.
 * O sqlite guarda o endereço dos dois na tabela de syscalls. O processo do
 * RetroArch não é root, então o fchown não chega a ser chamado; um readlink
 * que falha é o que o sqlite trata como "este caminho não é um symlink". */
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
