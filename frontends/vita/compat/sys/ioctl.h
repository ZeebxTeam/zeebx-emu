/* A newlib da Vita não tem sys/ioctl.h. O VFS Unix do SQLite inclui o
 * cabeçalho sempre; a chamada em si só entra com locking estilo Linux, que
 * este alvo não liga. A declaração basta para o include resolver. */
#ifndef _SYS_IOCTL_H
#define _SYS_IOCTL_H

#ifdef __cplusplus
extern "C" {
#endif

int ioctl(int fd, unsigned long request, ...);

#ifdef __cplusplus
}
#endif

#endif
