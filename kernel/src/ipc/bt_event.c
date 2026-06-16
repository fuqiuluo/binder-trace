#include "bt_event.h"

#include <linux/cred.h>
#include <linux/errno.h>
#include <linux/ktime.h>
#include <linux/minmax.h>
#include <linux/poll.h>
#include <linux/sched.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/uidgid.h>

#include "bt_common.h"
#include "bt_sock.h"

#define BT_EVENT_RING_CAPACITY 1024U
#define BT_EVENT_RING_MASK (BT_EVENT_RING_CAPACITY - 1U)

static DEFINE_SPINLOCK(bt_event_lock);
static DECLARE_WAIT_QUEUE_HEAD(bt_event_waitq);
static struct bt_binder_event bt_event_ring[BT_EVENT_RING_CAPACITY];
static u64 bt_event_next_sequence = 1;

int bt_event_init(void)
{
    unsigned long flags;

    spin_lock_irqsave(&bt_event_lock, flags);
    memset(bt_event_ring, 0, sizeof(bt_event_ring));
    bt_event_next_sequence = 1;
    spin_unlock_irqrestore(&bt_event_lock, flags);

    bt_info("事件流已初始化: ring_capacity=%u\n", BT_EVENT_RING_CAPACITY);
    return 0;
}

void bt_event_cleanup(void)
{
    wake_up_interruptible_poll(&bt_event_waitq, EPOLLHUP);
    bt_info("事件流已清理\n");
}

void bt_event_socket_init(struct bt_sock *bs)
{
    if (!bs) {
        return;
    }

    bs->next_event_sequence = READ_ONCE(bt_event_next_sequence);
}

bool bt_event_has_pending(const struct bt_sock *bs)
{
    if (!bs) {
        return false;
    }

    return READ_ONCE(bs->next_event_sequence) != READ_ONCE(bt_event_next_sequence);
}

wait_queue_head_t *bt_event_wait_queue(void)
{
    return &bt_event_waitq;
}

static int bt_event_take_locked(struct bt_sock *bs, struct bt_binder_event *event)
{
    u64 cursor;
    u64 oldest;
    u64 next;
    u32 lost_before = 0;

    cursor = bs->next_event_sequence;
    next = bt_event_next_sequence;
    if (cursor == next) {
        return -EAGAIN;
    }

    oldest = next > BT_EVENT_RING_CAPACITY ? next - BT_EVENT_RING_CAPACITY : 1;
    if (cursor < oldest) {
        lost_before = (u32)(oldest - cursor);
        cursor = oldest;
    }

    *event = bt_event_ring[cursor & BT_EVENT_RING_MASK];
    event->lost_before += lost_before;
    bs->next_event_sequence = cursor + 1;
    return 0;
}

int bt_event_recv(struct bt_sock *bs, struct bt_binder_event *event, bool nonblock)
{
    unsigned long flags;
    int ret;

    if (!bs || !event) {
        return -EINVAL;
    }

    for (;;) {
        spin_lock_irqsave(&bt_event_lock, flags);
        ret = bt_event_take_locked(bs, event);
        spin_unlock_irqrestore(&bt_event_lock, flags);
        if (ret != -EAGAIN) {
            return ret;
        }

        if (nonblock) {
            return -EAGAIN;
        }

        ret = wait_event_interruptible(bt_event_waitq, bt_event_has_pending(bs));
        if (ret) {
            return ret;
        }
    }
}

void bt_event_emit_binder_transaction(
    const void *proc,
    const void *thread,
    const void *tr,
    int reply,
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
    bool payload_truncated)
{
    struct bt_binder_event event;
    unsigned long irq_flags;
    u64 sequence;
    __u32 inline_len = min_t(__u32, payload_len, BT_MAX_INLINE_PAYLOAD);

    event = (struct bt_binder_event){
        .timestamp_ns = ktime_get_ns(),
        .kind = BT_EVENT_KIND_BINDER_TRANSACTION,
        .pid = (__u32)task_pid_nr(current),
        .tgid = (__u32)task_tgid_nr(current),
        .uid = (__u32)__kuid_val(current_uid()),
        .reply = reply ? 1U : 0U,
        .lost_before = 0,
        .transaction = (unsigned long)tr,
        .proc = (unsigned long)proc,
        .thread = (unsigned long)thread,
        .extra_buffers_size = extra_buffers_size,
        .code = code,
        .flags = transaction_flags,
        .data_size = data_size,
        .offsets_size = offsets_size,
        .target_handle = target_handle,
        .sender_pid = sender_pid,
        .sender_euid = sender_euid,
        .payload_len = inline_len,
        .payload_truncated = payload_truncated ? 1U : 0U,
    };
    if (payload && inline_len) {
        memcpy(event.payload, payload, inline_len);
    }

    spin_lock_irqsave(&bt_event_lock, irq_flags);
    sequence = bt_event_next_sequence++;
    event.sequence = sequence;
    bt_event_ring[sequence & BT_EVENT_RING_MASK] = event;
    spin_unlock_irqrestore(&bt_event_lock, irq_flags);

    wake_up_interruptible_poll(&bt_event_waitq, EPOLLIN | EPOLLRDNORM);
}
