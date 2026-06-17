// SPDX-License-Identifier: GPL-2.0-only
#include <linux/init.h>
#include <linux/delay.h>
#include <linux/module.h>
#include <linux/types.h>

#include "bt_capture.h"
#include "bt_common.h"
#include "bt_hooks.h"
#include "bt_protocol.h"
#include "bt_symbols.h"
#include "bt_utils.h"
#include "hijack_arm64.h"
#include "inline_hook.h"

bool bt_preserve_bti = false;
module_param_named(preserve_bti, bt_preserve_bti, bool, 0444);
MODULE_PARM_DESC(
    preserve_bti,
    "保留 hooked text 页的 BTI guard 属性，并使用 RET X17 跳回原函数");

bool bt_hide_module = false;
module_param_named(hide_module, bt_hide_module, bool, 0444);
MODULE_PARM_DESC(
    hide_module,
    "隐藏内核模块的痕迹, 保证审计模块不会被恶意用户异常卸载!");

static int __init bt_kmod_init(void)
{
    int ret;

    bt_info("init\n");
    bt_info("inline hook 跳回策略: %s\n",
            bt_preserve_bti ? "保留 BTI，使用 RET X17" : "清理 PTE_GP，使用 BR X17");

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

    bt_info("CFI bypass patched functions: %d\n", cfi_bypass());

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

    if (bt_hide_module) {
        hide_module();
    }
    return 0;
}

static void __exit bt_kmod_exit(void)
{
    int ret;

    if (bt_hide_module) {
        show_module();
    }

    bt_info("exit: 正在恢复 hook 并等待活跃调用退出\n");
    ret = bt_binder_hooks_remove();
    if (ret) {
        bt_err("exit: 恢复 hook 失败: %d，模块不能安全卸载，阻塞退出避免入口跳到已释放代码\n",
               ret);
        for (;;) {
            ssleep(60);
        }
    }

    bt_protocol_cleanup();
    bt_capture_cleanup();
    wuwa_inlinehook_cleanup();
    bt_info("exit: 完成\n");
}

module_init(bt_kmod_init);
module_exit(bt_kmod_exit);

MODULE_AUTHOR("fuqiuluo");
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("binder-trace Android 内核模块后端");
MODULE_VERSION("0.1.0");
