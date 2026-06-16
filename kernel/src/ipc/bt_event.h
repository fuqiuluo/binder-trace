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
    unsigned long extra_buffers_size);

#endif /* BINDER_TRACE_KMOD_EVENT_H */
