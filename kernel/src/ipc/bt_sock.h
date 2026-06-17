/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef BINDER_TRACE_KMOD_SOCK_H
#define BINDER_TRACE_KMOD_SOCK_H

#include <linux/net.h>
#include <linux/types.h>
#include <net/sock.h>

struct bt_sock {
    struct sock sk;
    u32 version;
    pid_t owner_tgid;
    u64 next_event_sequence;
};

extern struct proto_ops bt_proto_ops;

int bt_sock_init(struct bt_sock *bs, pid_t owner_tgid);

#endif /* BINDER_TRACE_KMOD_SOCK_H */
