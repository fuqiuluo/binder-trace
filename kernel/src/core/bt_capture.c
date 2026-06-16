#include "bt_capture.h"

#include <linux/atomic.h>
#include <linux/cred.h>
#include <linux/errno.h>
#include <linux/sched.h>
#include <linux/spinlock.h>
#include <linux/uidgid.h>

#include "bt_common.h"

static struct bt_capture_config bt_capture_config;
static DEFINE_SPINLOCK(bt_capture_config_lock);

static atomic64_t bt_ioctl_hits = ATOMIC64_INIT(0);
static atomic64_t bt_copy_to_user_hits = ATOMIC64_INIT(0);
static atomic64_t bt_transaction_hits = ATOMIC64_INIT(0);
static atomic64_t bt_captured = ATOMIC64_INIT(0);
static atomic64_t bt_filtered = ATOMIC64_INIT(0);

int bt_capture_init(void)
{
    struct bt_capture_config config = {
        .enabled = 0,
        .point_mask = BT_CAPTURE_POINT_ALL,
        .tgid = 0,
        .pid = 0,
        .uid = 0,
        .uid_enabled = 0,
        .ioctl_cmd = 0,
        .ioctl_cmd_enabled = 0,
        .min_size = 0,
        .max_size = 0,
    };

    bt_capture_set_config(&config);
    bt_capture_clear_stats();
    return 0;
}

void bt_capture_cleanup(void)
{
    struct bt_capture_config config;

    bt_capture_get_config(&config);
    config.enabled = 0;
    bt_capture_set_config(&config);
}

int bt_capture_set_config(const struct bt_capture_config *config)
{
    struct bt_capture_config next;
    unsigned long flags;

    if (!config) {
        return -EINVAL;
    }

    next = *config;
    next.enabled = !!next.enabled;
    next.uid_enabled = !!next.uid_enabled;
    next.ioctl_cmd_enabled = !!next.ioctl_cmd_enabled;

    if (!next.point_mask) {
        next.point_mask = BT_CAPTURE_POINT_ALL;
    }

    next.point_mask &= BT_CAPTURE_POINT_ALL;
    if (!next.point_mask) {
        return -EINVAL;
    }

    if (next.max_size && next.min_size > next.max_size) {
        return -EINVAL;
    }

    spin_lock_irqsave(&bt_capture_config_lock, flags);
    bt_capture_config = next;
    spin_unlock_irqrestore(&bt_capture_config_lock, flags);

    bt_info("捕获配置已更新: enabled=%u point_mask=0x%x tgid=%d pid=%d uid_enabled=%u uid=%u\n",
            next.enabled,
            next.point_mask,
            next.tgid,
            next.pid,
            next.uid_enabled,
            next.uid);
    return 0;
}

void bt_capture_get_config(struct bt_capture_config *config)
{
    unsigned long flags;

    if (!config) {
        return;
    }

    spin_lock_irqsave(&bt_capture_config_lock, flags);
    *config = bt_capture_config;
    spin_unlock_irqrestore(&bt_capture_config_lock, flags);
}

void bt_capture_get_stats(struct bt_capture_stats *stats)
{
    if (!stats) {
        return;
    }

    stats->ioctl_hits = atomic64_read(&bt_ioctl_hits);
    stats->copy_to_user_hits = atomic64_read(&bt_copy_to_user_hits);
    stats->transaction_hits = atomic64_read(&bt_transaction_hits);
    stats->captured = atomic64_read(&bt_captured);
    stats->filtered = atomic64_read(&bt_filtered);
}

void bt_capture_clear_stats(void)
{
    atomic64_set(&bt_ioctl_hits, 0);
    atomic64_set(&bt_copy_to_user_hits, 0);
    atomic64_set(&bt_transaction_hits, 0);
    atomic64_set(&bt_captured, 0);
    atomic64_set(&bt_filtered, 0);
}

static void bt_capture_note_hit(__u32 point)
{
    if (point == BT_CAPTURE_POINT_IOCTL) {
        atomic64_inc(&bt_ioctl_hits);
    } else if (point == BT_CAPTURE_POINT_COPY_TO_USER) {
        atomic64_inc(&bt_copy_to_user_hits);
    } else if (point == BT_CAPTURE_POINT_TRANSACTION) {
        atomic64_inc(&bt_transaction_hits);
    }
}

bool bt_capture_should_trace(__u32 point, __u32 ioctl_cmd, size_t size)
{
    struct bt_capture_config config;
    unsigned long flags;
    kuid_t uid;

    bt_capture_note_hit(point);

    spin_lock_irqsave(&bt_capture_config_lock, flags);
    config = bt_capture_config;
    spin_unlock_irqrestore(&bt_capture_config_lock, flags);

    if (!config.enabled) {
        return false;
    }

    if (!(config.point_mask & point)) {
        atomic64_inc(&bt_filtered);
        return false;
    }

    if (config.tgid > 0 && task_tgid_nr(current) != config.tgid) {
        atomic64_inc(&bt_filtered);
        return false;
    }

    if (config.pid > 0 && task_pid_nr(current) != config.pid) {
        atomic64_inc(&bt_filtered);
        return false;
    }

    if (config.uid_enabled) {
        uid = current_uid();
        if (!uid_eq(uid, make_kuid(current_user_ns(), config.uid))) {
            atomic64_inc(&bt_filtered);
            return false;
        }
    }

    if (config.ioctl_cmd_enabled && point == BT_CAPTURE_POINT_IOCTL &&
        ioctl_cmd != config.ioctl_cmd) {
        atomic64_inc(&bt_filtered);
        return false;
    }

    if (config.min_size && size < config.min_size) {
        atomic64_inc(&bt_filtered);
        return false;
    }

    if (config.max_size && size > config.max_size) {
        atomic64_inc(&bt_filtered);
        return false;
    }

    atomic64_inc(&bt_captured);
    return true;
}
