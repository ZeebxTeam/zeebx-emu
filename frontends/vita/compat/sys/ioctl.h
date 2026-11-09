/* The Vita newlib has no sys/ioctl.h. SQLite's Unix VFS always includes the
 * header; the call itself only comes in with Linux-style locking, which this
 * target does not turn on. The declaration is enough for the include to resolve. */
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
