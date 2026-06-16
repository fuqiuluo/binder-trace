#ifndef BINDER_TRACE_KMOD_IPC_UAPI_H
#define BINDER_TRACE_KMOD_IPC_UAPI_H

#include <linux/ioctl.h>
#include <linux/types.h>

/*
 * 控制面使用自定义 socket 协议族承载，不创建 /dev 节点。
 * 协议族编号由模块加载时动态分配；用户态从 AF_DECnet 开始探测，找到
 * SOCK_STREAM 返回 ENOKEY、SOCK_RAW 可创建的 family 后再发送 ioctl。
 */

#define BT_ABI_VERSION 1U
#define BT_IOC_MAGIC 'B'
#define BT_DRIVER_FEATURE_MAGIC 0x4254524143453031ULL
#define BT_DRIVER_FEATURE_NAME "binder-trace"

#define BT_FEATURE_CONTROL_SOCKET (1U << 0)
#define BT_FEATURE_EVENT_STREAM (1U << 1)

#define BT_EVENT_KIND_BINDER_TRANSACTION 1U

#define BT_CAPTURE_POINT_IOCTL (1U << 0)
#define BT_CAPTURE_POINT_COPY_TO_USER (1U << 1)
#define BT_CAPTURE_POINT_TRANSACTION (1U << 2)
#define BT_CAPTURE_POINT_ALL \
    (BT_CAPTURE_POINT_IOCTL | BT_CAPTURE_POINT_COPY_TO_USER | BT_CAPTURE_POINT_TRANSACTION)

struct bt_abi_version {
    __u32 version;
    __u32 _reserved;
};

struct bt_driver_feature {
    __u64 magic;
    __u32 abi_version;
    __u32 feature_flags;
    char name[16];
};

struct bt_capture_config {
    __u32 enabled;
    __u32 point_mask;
    __s32 tgid;
    __s32 pid;
    __u32 uid;
    __u32 uid_enabled;
    __u32 ioctl_cmd;
    __u32 ioctl_cmd_enabled;
    __u64 min_size;
    __u64 max_size;
};

struct bt_capture_stats {
    __u64 ioctl_hits;
    __u64 copy_to_user_hits;
    __u64 transaction_hits;
    __u64 captured;
    __u64 filtered;
};

/*
 * Binder 事件流通过同一个自定义 socket 的 recvmsg 读取。
 *
 * 这里只暴露跨 Android common 5.10/5.15/6.1/6.6/6.12 稳定的信息：
 * 当前任务身份、binder_transaction() 的 reply 标记和入口指针值。
 * 不在内核模块里解引用 binder_proc/binder_thread/binder_transaction_data，
 * 后续如果要解析字段，必须按目标内核版本显式适配。
 */
struct bt_binder_event {
    __u64 sequence;
    __u64 timestamp_ns;
    __u32 kind;
    __u32 pid;
    __u32 tgid;
    __u32 uid;
    __u32 reply;
    __u32 lost_before;
    __u64 transaction;
    __u64 proc;
    __u64 thread;
    __u64 extra_buffers_size;
};

#define BT_IOC_GET_ABI_VERSION _IOR(BT_IOC_MAGIC, 0x00, struct bt_abi_version)
#define BT_IOC_SET_CONFIG _IOW(BT_IOC_MAGIC, 0x01, struct bt_capture_config)
#define BT_IOC_GET_CONFIG _IOR(BT_IOC_MAGIC, 0x02, struct bt_capture_config)
#define BT_IOC_GET_STATS _IOR(BT_IOC_MAGIC, 0x03, struct bt_capture_stats)
#define BT_IOC_CLEAR_STATS _IO(BT_IOC_MAGIC, 0x04)
#define BT_IOC_GET_FEATURE _IOR(BT_IOC_MAGIC, 0x05, struct bt_driver_feature)

#define BT_IOC_MAX_NR 0x05

#endif /* BINDER_TRACE_KMOD_IPC_UAPI_H */
