#include <linux/init.h>
#include <linux/module.h>
#include <linux/types.h>

#include "bt_capture.h"
#include "bt_common.h"
#include "bt_hooks.h"
#include "bt_protocol.h"
#include "bt_symbols.h"
#include "hijack_arm64.h"
#include "inline_hook.h"

static int __init bt_kmod_init(void)
{
    int ret;

    bt_info("init\n");

    ret = bt_capture_init();
    if (ret) {
        bt_err("捕获配置初始化失败: %d\n", ret);
        return ret;
    }

    ret = init_arch();
    if (ret) {
        bt_err("init_arch 初始化失败: %d\n", ret);
        bt_capture_cleanup();
        return ret;
    }

    ret = wuwa_inlinehook_init();
    if (ret) {
        bt_err("inline hook 初始化失败: %d\n", ret);
        bt_capture_cleanup();
        return ret;
    }

    ret = bt_protocol_init();
    if (ret) {
        bt_err("控制协议族初始化失败: %d\n", ret);
        wuwa_inlinehook_cleanup();
        bt_capture_cleanup();
        return ret;
    }

    ret = bt_binder_symbols_init();
    if (ret) {
        bt_err("初始化 Binder trace 符号失败: %d\n", ret);
        bt_protocol_cleanup();
        wuwa_inlinehook_cleanup();
        bt_capture_cleanup();
        return ret;
    }

    ret = bt_binder_hooks_install();
    if (ret) {
        bt_err("安装 Binder hook 失败: %d\n", ret);
        bt_protocol_cleanup();
        wuwa_inlinehook_cleanup();
        bt_capture_cleanup();
        return ret;
    }

    /*
     * 当前 hook 函数用普通 C 调用 backup，binder_ioctl 可能长时间阻塞并保留
     * 返回到本模块文本段的栈帧。直到 hook 改为真正 tail-call 形式前，热卸载
     * 无法做到可靠安全；这里自持有模块引用，让普通 rmmod 返回 busy。
     */
    if (!try_module_get(THIS_MODULE)) {
        bt_err("自持有模块引用失败\n");
        bt_binder_hooks_remove();
        bt_protocol_cleanup();
        wuwa_inlinehook_cleanup();
        bt_capture_cleanup();
        return -ENODEV;
    }
    bt_info("已禁止普通 rmmod 热卸载，避免 Binder 长阻塞路径返回到已卸载模块\n");

    return 0;
}

static void __exit bt_kmod_exit(void)
{
    bt_info("exit\n");
    bt_capture_cleanup();
    bt_binder_hooks_remove();
    bt_protocol_cleanup();
    wuwa_inlinehook_cleanup();
}

module_init(bt_kmod_init);
module_exit(bt_kmod_exit);

MODULE_AUTHOR("fuqiuluo");
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("binder-trace Android 内核模块后端");
MODULE_VERSION("0.1.0");
