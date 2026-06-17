// SPDX-License-Identifier: GPL-2.0-only
#include "bt_protocol.h"

#include <linux/capability.h>
#include <linux/cred.h>
#include <linux/errno.h>
#include <linux/net.h>
#include <linux/socket.h>
#include <net/sock.h>

#include "bt_common.h"
#include "bt_event.h"
#include "bt_ipc_uapi.h"
#include "bt_sock.h"

static int bt_free_family = AF_DECnet;
static bool bt_proto_registered;
static bool bt_family_registered;

static struct proto bt_proto = {
    .name = "BT_CTL",
    .owner = THIS_MODULE,
    .obj_size = sizeof(struct bt_sock),
};

static int bt_sock_create(struct net *net, struct socket *sock, int protocol, int kern)
{
    struct sock *sk;
    struct bt_sock *bs;
    kuid_t root_uid = GLOBAL_ROOT_UID;

    if (!uid_eq(current_uid(), root_uid) || !capable(CAP_SYS_ADMIN)) {
        bt_warn("只有 root 且具备 CAP_SYS_ADMIN 才可以创建 binder-trace 控制 socket\n");
        return -EACCES;
    }

    if (sock->type != SOCK_RAW) {
        return -ENOKEY;
    }

    sock->state = SS_UNCONNECTED;
    sk = sk_alloc(net, bt_free_family, GFP_KERNEL, &bt_proto, kern);
    if (!sk) {
        return -ENOBUFS;
    }

    sock->ops = &bt_proto_ops;
    sock_init_data(sock, sk);

    bs = (struct bt_sock *)sk;
    if (bt_sock_init(bs, task_tgid_nr(current))) {
        sock_release(sock);
        return -ENOMEM;
    }

    return 0;
}

static struct net_proto_family bt_family_ops = {
    .family = PF_DECnet,
    .create = bt_sock_create,
    .owner = THIS_MODULE,
};

static int bt_register_free_family(void)
{
    int err = -EAFNOSUPPORT;
    int family;

    for (family = bt_free_family; family < NPROTO; family++) {
        bt_family_ops.family = family;
        err = sock_register(&bt_family_ops);
        if (err) {
            continue;
        }

        bt_free_family = family;
        bt_proto_ops.family = family;
        bt_family_registered = true;
        bt_info("控制协议族已注册: family=%d max_cmd=%u\n", family, BT_IOC_MAX_NR);
        return 0;
    }

    bt_err("找不到可用的控制协议族: %d\n", err);
    return err;
}

int bt_protocol_init(void)
{
    int ret;

    ret = bt_event_init();
    if (ret) {
        bt_err("事件流初始化失败: %d\n", ret);
        return ret;
    }

    ret = proto_register(&bt_proto, 1);
    if (ret) {
        bt_err("注册控制 proto 失败: %d\n", ret);
        bt_event_cleanup();
        return ret;
    }
    bt_proto_registered = true;

    ret = bt_register_free_family();
    if (ret) {
        proto_unregister(&bt_proto);
        bt_proto_registered = false;
        bt_event_cleanup();
        return ret;
    }

    return 0;
}

void bt_protocol_cleanup(void)
{
    if (bt_family_registered) {
        sock_unregister(bt_free_family);
        bt_family_registered = false;
    }

    if (bt_proto_registered) {
        proto_unregister(&bt_proto);
        bt_proto_registered = false;
    }

    bt_event_cleanup();
    bt_info("控制协议族已清理\n");
}
