/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef BINDER_TRACE_KMOD_CAPTURE_H
#define BINDER_TRACE_KMOD_CAPTURE_H

#include <linux/types.h>

#include "bt_ipc_uapi.h"

int bt_capture_init(void);
void bt_capture_cleanup(void);

int bt_capture_set_config(const struct bt_capture_config *config);
void bt_capture_get_config(struct bt_capture_config *config);
void bt_capture_get_stats(struct bt_capture_stats *stats);
void bt_capture_clear_stats(void);

bool bt_capture_should_trace(__u32 point, __u32 ioctl_cmd, size_t size);

#endif /* BINDER_TRACE_KMOD_CAPTURE_H */
