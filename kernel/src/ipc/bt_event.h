/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef BINDER_TRACE_KMOD_EVENT_H
#define BINDER_TRACE_KMOD_EVENT_H

#include <linux/types.h>
#include <linux/wait.h>

#include "bt_ipc_uapi.h"

struct bt_sock;

int bt_event_init(void);
void bt_event_cleanup(void);

void bt_event_socket_init(struct bt_sock *bs);
bool bt_event_has_pending(const struct bt_sock *bs);
wait_queue_head_t *bt_event_wait_queue(void);
int bt_event_recv(struct bt_sock *bs, struct bt_binder_event *event, bool nonblock);

void bt_event_emit_binder_transaction(
    const void *proc,
    const void *thread,
    const void *tr,
    int reply,
    __u32 transaction_debug_id,
    __u32 reply_to_debug_id,
    unsigned long extra_buffers_size,
    __u32 code,
    __u32 transaction_flags,
    __u64 data_size,
    __u64 offsets_size,
    __u32 target_handle,
    __u32 sender_pid,
    __u32 sender_euid,
    const __u8 *payload,
    __u32 payload_len,
    bool payload_truncated);

#endif /* BINDER_TRACE_KMOD_EVENT_H */
