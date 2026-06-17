#include <linux/err.h>
#include <linux/errno.h>
#include <linux/atomic.h>
#include <linux/compiler.h>
#include <linux/delay.h>
#include <linux/fs.h>
#include <linux/kernel.h>
#include <linux/list.h>
#include <linux/rcupdate.h>
#include <linux/rbtree.h>
#include <linux/string.h>
#include <linux/types.h>
#include <linux/uaccess.h>
#include <uapi/linux/android/binder.h>

#include "bt_capture.h"
#include "bt_common.h"
#include "bt_event.h"
#include "bt_hooks.h"
#include "bt_symbols.h"
#include "inline_hook.h"

struct binder_alloc;
struct binder_buffer;
struct binder_proc;
struct binder_thread;

typedef uintptr_t bt_binder_size_t;

/*
 * 只读取跨 Android common 5.10-6.12/mainline 相对稳定的 Binder 私有字段。
 * 这些不是 UAPI；读取失败或布局不匹配时只会让 debug id 为 0，用户态会降级
 * 为 unmatched，不影响原始 binder_transaction()。
 */
struct bt_binder_thread_layout {
    struct binder_proc *proc;
    struct rb_node rb_node;
    struct list_head waiting_thread_node;
    int pid;
    int looper;
    bool looper_need_return;
    struct binder_transaction *transaction_stack;
};

struct bt_binder_transaction_layout {
    int debug_id;
};

typedef long (*bt_binder_ioctl_fn)(struct file *filp, unsigned int cmd, unsigned long arg);
typedef unsigned long (*bt_binder_alloc_copy_user_to_buffer_fn)(
    struct binder_alloc *alloc,
    struct binder_buffer *buffer,
    bt_binder_size_t buffer_offset,
    const void __user *from,
    size_t bytes);
typedef void (*bt_binder_transaction_fn)(
    struct binder_proc *proc,
    struct binder_thread *thread,
    struct binder_transaction_data *tr,
    int reply,
    bt_binder_size_t extra_buffers_size);

struct bt_binder_hook_state {
    struct wuwa_inlinehook *binder_ioctl;
    struct wuwa_inlinehook *binder_alloc_copy_user_to_buffer;
    struct wuwa_inlinehook *binder_transaction;
    bt_binder_ioctl_fn binder_ioctl_backup;
    bt_binder_alloc_copy_user_to_buffer_fn binder_alloc_copy_user_to_buffer_backup;
    bt_binder_transaction_fn binder_transaction_backup;
};

static struct bt_binder_hook_state bt_binder_hooks;
static atomic_t bt_active_hook_calls = ATOMIC_INIT(0);
static bool bt_hooks_draining;

static void bt_hook_enter(void)
{
    atomic_inc(&bt_active_hook_calls);
}

static void bt_hook_leave(void)
{
    atomic_dec(&bt_active_hook_calls);
}

static void bt_wait_for_active_hooks(void)
{
    int loops = 0;

    while (atomic_read(&bt_active_hook_calls) > 0) {
        if ((loops++ % 50) == 0) {
            bt_info("等待活跃 hook 调用退出: active=%d\n",
                    atomic_read(&bt_active_hook_calls));
        }
        msleep(20);
    }
}

static struct binder_transaction *bt_read_transaction_stack(const struct binder_thread *thread)
{
    const struct bt_binder_thread_layout *layout;
    struct binder_transaction *transaction = NULL;

    if (!thread) {
        return NULL;
    }

    layout = (const struct bt_binder_thread_layout *)thread;
    if (copy_from_kernel_nofault(
            &transaction,
            &layout->transaction_stack,
            sizeof(transaction))) {
        return NULL;
    }

    return transaction;
}

static __u32 bt_read_transaction_debug_id(const struct binder_transaction *transaction)
{
    const struct bt_binder_transaction_layout *layout;
    int debug_id = 0;

    if (!transaction) {
        return 0;
    }

    layout = (const struct bt_binder_transaction_layout *)transaction;
    if (copy_from_kernel_nofault(&debug_id, &layout->debug_id, sizeof(debug_id))) {
        return 0;
    }
    if (debug_id <= 0) {
        return 0;
    }

    return (__u32)debug_id;
}

/*
 * 这里只拷贝发送方用户态 Parcel 的前缀，用于用户态解析 interface token。
 * copy_from_user() 失败时留空并继续调用原始 binder_transaction()。
 */
static __u32 bt_copy_transaction_payload(
    const struct binder_transaction_data *tr,
    __u8 payload[BT_MAX_INLINE_PAYLOAD],
    bool *payload_truncated)
{
    size_t inline_len;

    if (!tr || !payload || !payload_truncated) {
        return 0;
    }

    *payload_truncated = false;
    memset(payload, 0, BT_MAX_INLINE_PAYLOAD);

    if (!tr->data_size || !tr->data.ptr.buffer) {
        return 0;
    }

    inline_len = min_t(size_t, (size_t)tr->data_size, BT_MAX_INLINE_PAYLOAD);
    if (copy_from_user(
            payload,
            (const void __user *)(uintptr_t)tr->data.ptr.buffer,
            inline_len)) {
        *payload_truncated = true;
        memset(payload, 0, BT_MAX_INLINE_PAYLOAD);
        return 0;
    }

    *payload_truncated = tr->data_size > BT_MAX_INLINE_PAYLOAD;
    return (__u32)inline_len;
}

/*
 * 5.19 说明：Android common 当前没有标准 `android*-5.19*` ACK/GKI 分支。
 * 如果目标设备基于厂商 5.19 内核，需要用厂商源码里的
 * `drivers/android/binder.c` 和 `drivers/android/binder_alloc.c` 重新对照函数签名。
 */

/*
 * 内核来源：
 * - 5.10: https://android.googlesource.com/kernel/common/+/refs/heads/android12-5.10/drivers/android/binder.c
 * - 5.15: https://android.googlesource.com/kernel/common/+/refs/heads/android13-5.15/drivers/android/binder.c
 * - 6.1:  https://android.googlesource.com/kernel/common/+/refs/heads/android14-6.1/drivers/android/binder.c
 * - 6.6:  https://android.googlesource.com/kernel/common/+/refs/heads/android15-6.6/drivers/android/binder.c
 * - 6.12: https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/drivers/android/binder.c
 *
 * `binder_ioctl()` 在上述 ACK/GKI 分支里的签名保持为：
 *   static long binder_ioctl(struct file *filp, unsigned int cmd, unsigned long arg)
 *
 * 约束：这里不能解引用 `filp->private_data` 里的 `struct binder_proc`。该结构不是
 * UAPI，跨版本字段布局没有稳定 ABI；hook 层只采集入口参数，后续解析应建立显式
 * 版本适配。
 */
static __nocfi long bt_hook_binder_ioctl(struct file *filp, unsigned int cmd, unsigned long arg)
{
    long ret;

    bt_hook_enter();

    if (!READ_ONCE(bt_hooks_draining) &&
        bt_capture_should_trace(BT_CAPTURE_POINT_IOCTL, cmd, 0)) {
        bt_info_ratelimited("binder_ioctl filp=%px cmd=0x%x arg=0x%lx\n", filp, cmd, arg);
    }

    if (!bt_binder_hooks.binder_ioctl_backup) {
        bt_err("binder_ioctl backup 不存在\n");
        ret = -ENOSYS;
        goto out;
    }

    ret = bt_binder_hooks.binder_ioctl_backup(filp, cmd, arg);

out:
    bt_hook_leave();
    return ret;
}

/*
 * 内核来源：
 * - 5.10: https://android.googlesource.com/kernel/common/+/refs/heads/android12-5.10/drivers/android/binder_alloc.c
 * - 5.15: https://android.googlesource.com/kernel/common/+/refs/heads/android13-5.15/drivers/android/binder_alloc.c
 * - 6.1:  https://android.googlesource.com/kernel/common/+/refs/heads/android14-6.1/drivers/android/binder_alloc.c
 * - 6.6:  https://android.googlesource.com/kernel/common/+/refs/heads/android15-6.6/drivers/android/binder_alloc.c
 * - 6.12: https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/drivers/android/binder_alloc.c
 *
 * `binder_alloc_copy_user_to_buffer()` 在上述分支里的签名保持为：
 *   unsigned long binder_alloc_copy_user_to_buffer(struct binder_alloc *alloc,
 *       struct binder_buffer *buffer, binder_size_t buffer_offset,
 *       const void __user *from, size_t bytes)
 *
 * 5.10/5.15 内部使用 `kmap()`，6.1/6.6/6.12 内部改为 `kmap_local_page()`，
 * 但入口参数和返回值不变。hook 层只读取入口参数，不进入 `binder_alloc` 或
 * `binder_buffer` 内部字段，避免跨版本字段变化造成崩溃。
 */
static __nocfi unsigned long bt_hook_binder_alloc_copy_user_to_buffer(
    struct binder_alloc *alloc,
    struct binder_buffer *buffer,
    bt_binder_size_t buffer_offset,
    const void __user *from,
    size_t bytes)
{
    unsigned long ret;

    bt_hook_enter();

    if (!READ_ONCE(bt_hooks_draining) &&
        bt_capture_should_trace(BT_CAPTURE_POINT_COPY_TO_USER, 0, bytes)) {
        bt_info_ratelimited(
            "binder_alloc_copy_user_to_buffer alloc=%px buffer=%px off=0x%lx from=%px bytes=%zu\n",
            alloc,
            buffer,
            (unsigned long)buffer_offset,
            from,
            bytes);
    }

    if (!bt_binder_hooks.binder_alloc_copy_user_to_buffer_backup) {
        bt_err("binder_alloc_copy_user_to_buffer backup 不存在\n");
        ret = bytes;
        goto out;
    }

    ret = bt_binder_hooks.binder_alloc_copy_user_to_buffer_backup(
        alloc,
        buffer,
        buffer_offset,
        from,
        bytes);

out:
    bt_hook_leave();
    return ret;
}

/*
 * 内核来源：
 * - 5.10: https://android.googlesource.com/kernel/common/+/refs/heads/android12-5.10/drivers/android/binder.c
 * - 5.15: https://android.googlesource.com/kernel/common/+/refs/heads/android13-5.15/drivers/android/binder.c
 * - 6.1:  https://android.googlesource.com/kernel/common/+/refs/heads/android14-6.1/drivers/android/binder.c
 * - 6.6:  https://android.googlesource.com/kernel/common/+/refs/heads/android15-6.6/drivers/android/binder.c
 * - 6.12: https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/drivers/android/binder.c
 *
 * `binder_transaction()` 在上述分支里的签名保持为：
 *   static void binder_transaction(struct binder_proc *proc,
 *       struct binder_thread *thread, struct binder_transaction_data *tr,
 *       int reply, binder_size_t extra_buffers_size)
 *
 * 这是静态函数，部分设备可能没有 kallsyms 记录，所以安装时只作为可选 hook。
 */
static __nocfi void bt_hook_binder_transaction(
    struct binder_proc *proc,
    struct binder_thread *thread,
    struct binder_transaction_data *tr,
    int reply,
    bt_binder_size_t extra_buffers_size)
{
    __u8 payload[BT_MAX_INLINE_PAYLOAD] = {0};
    struct binder_transaction *stack_before = NULL;
    __u32 payload_len = 0;
    __u32 transaction_debug_id = 0;
    __u32 reply_to_debug_id = 0;
    bool payload_truncated = false;
    bool should_trace = false;

    bt_hook_enter();

    should_trace = !READ_ONCE(bt_hooks_draining) &&
        bt_capture_should_trace(BT_CAPTURE_POINT_TRANSACTION, 0, (size_t)extra_buffers_size);
    if (should_trace) {
        stack_before = bt_read_transaction_stack(thread);
        if (reply) {
            reply_to_debug_id = bt_read_transaction_debug_id(stack_before);
        }
        payload_len = bt_copy_transaction_payload(tr, payload, &payload_truncated);
    }

    if (!bt_binder_hooks.binder_transaction_backup) {
        bt_err("binder_transaction backup 不存在\n");
        goto out;
    }

    bt_binder_hooks.binder_transaction_backup(proc, thread, tr, reply, extra_buffers_size);

    if (should_trace && !reply && tr && !(tr->flags & TF_ONE_WAY)) {
        struct binder_transaction *stack_after = bt_read_transaction_stack(thread);

        if (stack_after && stack_after != stack_before) {
            transaction_debug_id = bt_read_transaction_debug_id(stack_after);
        }
    }

    if (should_trace) {
        bt_event_emit_binder_transaction(
            proc,
            thread,
            tr,
            reply,
            transaction_debug_id,
            reply_to_debug_id,
            (unsigned long)extra_buffers_size,
            tr ? tr->code : 0,
            tr ? tr->flags : 0,
            tr ? tr->data_size : 0,
            tr ? tr->offsets_size : 0,
            tr ? tr->target.handle : 0,
            tr ? (__u32)tr->sender_pid : 0,
            tr ? (__u32)tr->sender_euid : 0,
            payload,
            payload_len,
            payload_truncated);
        bt_info_ratelimited(
            "binder_transaction proc=%px thread=%px tr=%px reply=%d code=%u txn_dbg=%u reply_to=%u size=0x%llx extra=0x%lx\n",
            proc,
            thread,
            tr,
            reply,
            tr ? tr->code : 0,
            transaction_debug_id,
            reply_to_debug_id,
            tr ? (unsigned long long)tr->data_size : 0,
            (unsigned long)extra_buffers_size);
    }

out:
    bt_hook_leave();
}

static int bt_install_required_hook(
    const char *name,
    unsigned long address,
    void *replacement,
    void **backup,
    struct wuwa_inlinehook **hook)
{
    *hook = wuwa_install_hook((void *)address, replacement, backup);
    if (IS_ERR(*hook)) {
        int ret = PTR_ERR(*hook);
        *hook = NULL;
        *backup = NULL;
        bt_err("安装 %s hook 失败: %d\n", name, ret);
        return ret;
    }

    bt_info("已安装 %s hook: target=0x%lx backup=%px\n", name, address, *backup);
    return 0;
}

static void bt_install_optional_hook(
    const char *name,
    unsigned long address,
    void *replacement,
    void **backup,
    struct wuwa_inlinehook **hook)
{
    int ret;

    if (!address) {
        bt_warn("跳过 %s hook: 符号不存在\n", name);
        return;
    }

    ret = bt_install_required_hook(name, address, replacement, backup, hook);
    if (ret) {
        bt_warn("跳过 %s hook: 安装失败 %d\n", name, ret);
    }
}

int bt_binder_hooks_install(void)
{
    int ret;

    ret = bt_install_required_hook(
        "binder_ioctl",
        bt_binder_symbols.binder_ioctl,
        bt_hook_binder_ioctl,
        (void **)&bt_binder_hooks.binder_ioctl_backup,
        &bt_binder_hooks.binder_ioctl);
    if (ret) {
        return ret;
    }

    ret = bt_install_required_hook(
        "binder_alloc_copy_user_to_buffer",
        bt_binder_symbols.binder_alloc_copy_user_to_buffer,
        bt_hook_binder_alloc_copy_user_to_buffer,
        (void **)&bt_binder_hooks.binder_alloc_copy_user_to_buffer_backup,
        &bt_binder_hooks.binder_alloc_copy_user_to_buffer);
    if (ret) {
        bt_binder_hooks_remove();
        return ret;
    }

    bt_install_optional_hook(
        "binder_transaction",
        bt_binder_symbols.binder_transaction,
        bt_hook_binder_transaction,
        (void **)&bt_binder_hooks.binder_transaction_backup,
        &bt_binder_hooks.binder_transaction);

    return 0;
}

static void bt_disable_hook(
    const char *name,
    struct wuwa_inlinehook **hook)
{
    int ret;

    if (!*hook) {
        return;
    }

    ret = wuwa_disable_hook(*hook);
    if (ret) {
        bt_err("移除 %s hook 失败: %d\n", name, ret);
    } else {
        bt_info("已恢复 %s hook 入口\n", name);
    }
}

static void bt_free_disabled_hook(
    struct wuwa_inlinehook **hook,
    void **backup)
{
    if (*hook) {
        wuwa_free_hook(*hook);
    }

    *hook = NULL;
    *backup = NULL;
}

void bt_binder_hooks_remove(void)
{
    WRITE_ONCE(bt_hooks_draining, true);

    bt_disable_hook(
        "binder_transaction",
        &bt_binder_hooks.binder_transaction);
    bt_disable_hook(
        "binder_alloc_copy_user_to_buffer",
        &bt_binder_hooks.binder_alloc_copy_user_to_buffer);
    bt_disable_hook(
        "binder_ioctl",
        &bt_binder_hooks.binder_ioctl);

    synchronize_rcu_tasks();
    bt_wait_for_active_hooks();

    bt_free_disabled_hook(
        &bt_binder_hooks.binder_transaction,
        (void **)&bt_binder_hooks.binder_transaction_backup);
    bt_free_disabled_hook(
        &bt_binder_hooks.binder_alloc_copy_user_to_buffer,
        (void **)&bt_binder_hooks.binder_alloc_copy_user_to_buffer_backup);
    bt_free_disabled_hook(
        &bt_binder_hooks.binder_ioctl,
        (void **)&bt_binder_hooks.binder_ioctl_backup);
}
