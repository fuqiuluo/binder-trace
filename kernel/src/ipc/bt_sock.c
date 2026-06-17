// SPDX-License-Identifier: GPL-2.0-only
#include "bt_sock.h"

#include <linux/capability.h>
#include <linux/cred.h>
#include <linux/errno.h>
#include <linux/poll.h>
#include <linux/socket.h>
#include <linux/uio.h>
#include <linux/uidgid.h>
#include <linux/version.h>
#include <net/sock.h>

#include "bt_common.h"
#include "bt_control.h"
#include "bt_event.h"

#define BT_SOCK_VERSION 1U

static bool bt_sock_is_owner(const struct bt_sock *bs)
{
    kuid_t root_uid = GLOBAL_ROOT_UID;

    return bs &&
           uid_eq(current_uid(), root_uid) &&
           capable(CAP_SYS_ADMIN) &&
           bs->owner_tgid == task_tgid_nr(current);
}

int bt_sock_init(struct bt_sock *bs, pid_t owner_tgid)
{
    if (!bs) {
        return -EINVAL;
    }

    bs->version = BT_SOCK_VERSION;
    bs->owner_tgid = owner_tgid;
    bt_event_socket_init(bs);
    bt_info("控制 socket 已初始化: owner_tgid=%d\n", owner_tgid);
    return 0;
}

static void bt_sock_release_state(struct bt_sock *bs)
{
    if (!bs) {
        return;
    }

    bs->version = 0;
    bs->owner_tgid = 0;
    bs->next_event_sequence = 0;
    bt_info("控制 socket 已释放\n");
}

static int bt_sock_release(struct socket *sock)
{
    struct sock *sk;
    struct bt_sock *bs;

    if (!sock) {
        return 0;
    }

    sk = sock->sk;
    if (!sk) {
        return 0;
    }

    bs = (struct bt_sock *)sk;
    bt_sock_release_state(bs);
    sock_orphan(sk);
    sock->sk = NULL;
    sock_put(sk);
    return 0;
}

static int bt_sock_ioctl(struct socket *sock, unsigned int cmd, unsigned long arg)
{
    struct bt_sock *bs;

    if (!sock || !sock->sk) {
        return -ENOTTY;
    }

    bs = (struct bt_sock *)sock->sk;
    if (bs->version != BT_SOCK_VERSION) {
        return -ENOTTY;
    }

    if (!bt_sock_is_owner(bs)) {
        bt_warn("拒绝非 owner 进程使用控制 socket: owner=%d current=%d\n",
                bs->owner_tgid,
                task_tgid_nr(current));
        return -EACCES;
    }

    return bt_control_dispatch(cmd, arg);
}

static __poll_t bt_sock_poll(struct file *file, struct socket *sock, struct poll_table_struct *wait)
{
    struct bt_sock *bs;

    if (!sock || !sock->sk) {
        return EPOLLERR;
    }

    bs = (struct bt_sock *)sock->sk;
    poll_wait(file, bt_event_wait_queue(), wait);

    if (bt_event_has_pending(bs)) {
        return EPOLLIN | EPOLLRDNORM;
    }

    return 0;
}

static int bt_sock_setsockopt(
    struct socket *sock,
    int level,
    int optname,
    sockptr_t optval,
    unsigned int optlen)
{
    return -ENOPROTOOPT;
}

static int bt_sock_getsockopt(
    struct socket *sock,
    int level,
    int optname,
    char __user *optval,
    int __user *optlen)
{
    return -ENOPROTOOPT;
}

static int bt_sock_bind(struct socket *sock, struct sockaddr *saddr, int len)
{
    return -EOPNOTSUPP;
}

static int bt_sock_connect(struct socket *sock, struct sockaddr *saddr, int len, int flags)
{
    return -EOPNOTSUPP;
}

static int bt_sock_getname(struct socket *sock, struct sockaddr *saddr, int peer)
{
    return -EOPNOTSUPP;
}

static int bt_sock_recvmsg(struct socket *sock, struct msghdr *msg, size_t len, int flags)
{
    struct bt_sock *bs;
    struct bt_binder_event event;
    int ret;

    if (!sock || !sock->sk || !msg) {
        return -EINVAL;
    }

    if (len < sizeof(event)) {
        return -EINVAL;
    }

    bs = (struct bt_sock *)sock->sk;
    if (bs->version != BT_SOCK_VERSION) {
        return -EINVAL;
    }

    if (!bt_sock_is_owner(bs)) {
        return -EACCES;
    }

    ret = bt_event_recv(bs, &event, !!(flags & MSG_DONTWAIT));
    if (ret) {
        return ret;
    }

    ret = memcpy_to_msg(msg, &event, sizeof(event));
    if (ret) {
        return ret;
    }

    return sizeof(event);
}

static int bt_sock_sendmsg(struct socket *sock, struct msghdr *msg, size_t len)
{
    return -EOPNOTSUPP;
}

static int bt_sock_socketpair(struct socket *sock1, struct socket *sock2)
{
    return -EOPNOTSUPP;
}

#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 12, 0)
static int bt_sock_accept(struct socket *sock, struct socket *newsock, struct proto_accept_arg *arg)
{
    return -EOPNOTSUPP;
}
#else
static int bt_sock_accept(struct socket *sock, struct socket *newsock, int flags, bool kern)
{
    return -EOPNOTSUPP;
}
#endif

static int bt_sock_listen(struct socket *sock, int backlog)
{
    return -EOPNOTSUPP;
}

static int bt_sock_shutdown(struct socket *sock, int how)
{
    return -EOPNOTSUPP;
}

static int bt_sock_mmap(struct file *file, struct socket *sock, struct vm_area_struct *vma)
{
    return -ENODEV;
}

struct proto_ops bt_proto_ops = {
    .family = PF_DECnet,
    .owner = THIS_MODULE,
    .release = bt_sock_release,
    .bind = bt_sock_bind,
    .connect = bt_sock_connect,
    .socketpair = bt_sock_socketpair,
    .accept = bt_sock_accept,
    .getname = bt_sock_getname,
    .poll = bt_sock_poll,
    .ioctl = bt_sock_ioctl,
    .listen = bt_sock_listen,
    .shutdown = bt_sock_shutdown,
    .setsockopt = bt_sock_setsockopt,
    .getsockopt = bt_sock_getsockopt,
    .sendmsg = bt_sock_sendmsg,
    .recvmsg = bt_sock_recvmsg,
    .mmap = bt_sock_mmap,
};
