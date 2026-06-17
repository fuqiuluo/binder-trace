/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef BINDER_TRACE_KMOD_COMMON_H
#define BINDER_TRACE_KMOD_COMMON_H

#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/types.h>

#define BT_LOG_PREFIX "[binder-trace] "

#define bt_info(fmt, ...) pr_info(BT_LOG_PREFIX fmt, ##__VA_ARGS__)
#define bt_info_ratelimited(fmt, ...) pr_info_ratelimited(BT_LOG_PREFIX fmt, ##__VA_ARGS__)
#define bt_warn(fmt, ...) pr_warn(BT_LOG_PREFIX fmt, ##__VA_ARGS__)
#define bt_err(fmt, ...) pr_err(BT_LOG_PREFIX fmt, ##__VA_ARGS__)
#define bt_debug(fmt, ...) pr_debug(BT_LOG_PREFIX fmt, ##__VA_ARGS__)

#ifdef BT_TRACE_LOG
#define bt_trace(fmt, ...) pr_warn(BT_LOG_PREFIX fmt, ##__VA_ARGS__)
#else
#define bt_trace(fmt, ...)
#endif

/* 兼容从 android-wuwa 搬入的 hook 基础设施，后续可再做纯机械 rename。 */
#define wuwa_info bt_info
#define wuwa_warn bt_warn
#define wuwa_err bt_err
#define wuwa_debug bt_debug
#define wuwa_trace bt_trace

extern bool bt_preserve_bti;

#endif /* BINDER_TRACE_KMOD_COMMON_H */
