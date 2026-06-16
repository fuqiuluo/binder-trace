/**
 * @file inline_hook.h
 * @brief ARM64 inline hook 框架，用于运行时拦截内核函数。
 *
 * 该框架会在目标函数入口写入跳转指令，把执行流导向替换函数，并生成
 * backup trampoline 供替换函数继续调用原始逻辑。
 *
 * @warning 被 hook 的函数必须满足:
 *          - __nocfi: 避开 CFI 检查
 *          - noinline: 防止编译器内联
 *
 * @example 基本用法
 * @code
 * // 1. 定义原函数和替换函数。
 * __nocfi noinline void original_func(int arg) {
 *     printk("原函数: %d\n", arg);
 * }
 *
 * static typeof(original_func)* original_func_backup = NULL;
 *
 * __nocfi void hook_func(int arg) {
 *     printk("已 hook，arg=%d\n", arg);
 *     // 继续调用原始逻辑。
 *     if (original_func_backup) {
 *         original_func_backup(arg + 1);
 *     }
 * }
 *
 * // 2. 安装 hook。
 * struct wuwa_inlinehook* hook;
 * hook = wuwa_install_hook(original_func, hook_func, (void**)&original_func_backup);
 * if (IS_ERR(hook)) {
 *     printk("hook 失败: %ld\n", PTR_ERR(hook));
 *     return PTR_ERR(hook);
 * }
 *
 * // 3. 测试 hook。
 * original_func(42);  // 实际会进入 hook_func。
 *
 * // 4. 移除 hook。
 * wuwa_remove_hook(hook);
 * @endcode
 */

#ifndef BINDER_TRACE_KMOD_INLINE_HOOK_H
#define BINDER_TRACE_KMOD_INLINE_HOOK_H

#include <linux/types.h>
#include <linux/err.h>

/**
 * @brief trampoline 指令数量，用于保存原函数入口。
 */
#define TRAMPOLINE_NUM (8)

/**
 * @brief 重定位指令数量，用于保存重定位后的原始代码。
 */
#define RELOCATE_INST_NUM (TRAMPOLINE_NUM * 8)

/**
 * @brief hook 地址信息。
 *
 * 记录一个 hook 点相关的目标地址、替换地址、解析后地址和 backup 地址。
 */
struct hook_address_info {
    uintptr_t target_func;      /**< 调用方传入的目标函数地址。 */
    uintptr_t replacement_func; /**< 替换函数地址。 */
    uintptr_t resolved_addr;    /**< 解析跳转链后的实际入口地址。 */
    uintptr_t backup_addr;      /**< backup 代码存放地址。 */
};

/**
 * @brief hook 指令缓存。
 *
 * 保存原始入口指令、跳转指令和重定位后的原始代码。
 */
struct hook_instruction_cache {
    int saved_count;        /**< 已保存的原始指令数量。 */
    int trampoline_count;   /**< trampoline 指令数量。 */
    int relocated_count;    /**< 重定位指令数量。 */

    /** 原函数入口指令，用于卸载 hook 时恢复。 */
    uint32_t saved_insns[TRAMPOLINE_NUM] __attribute__((aligned(8)));

    /** trampoline 指令，用于跳转到替换函数。 */
    uint32_t trampoline_insns[TRAMPOLINE_NUM] __attribute__((aligned(8)));

    /** 重定位后的原始指令，backup 函数会执行这些指令。 */
    uint32_t relocated_insns[RELOCATE_INST_NUM] __attribute__((aligned(8)));
};

/**
 * @brief inline hook 上下文。
 *
 * 框架内部维护地址和指令数据；调用方不应该直接修改。
 */
struct wuwa_inlinehook {
    struct hook_address_info addr;  /**< 地址信息。 */
    struct hook_instruction_cache insn; /**< 指令缓存。 */
};

#if defined(INLINE_HOOK)

/**
 * @brief 初始化 inline hook 框架。
 *
 * 模块初始化时调用。
 *
 * @return 成功返回 0，失败返回负 errno。
 */
int wuwa_inlinehook_init(void);

/**
 * @brief 清理 inline hook 框架。
 *
 * 模块卸载时调用，释放相关资源。
 */
void wuwa_inlinehook_cleanup(void);

/**
 * @brief 安装 inline hook。
 *
 * 运行时修改目标函数入口代码，把执行流重定向到替换函数，并生成可继续
 * 调用原始逻辑的 backup 函数。
 *
 * @param target 目标函数指针，必须满足 __nocfi noinline。
 * @param replace 替换函数指针，必须满足 __nocfi。
 * @param backup 输出参数，返回与目标函数同类型的 backup 函数指针。
 *
 * @return 成功返回 hook 结构体指针，失败返回 ERR_PTR(error_code)。
 *
 * @note 常见错误:
 *       -EINVAL: 参数非法或为空。
 *       -EFAULT: 无法访问目标函数对应页表项。
 *       其他: 内存分配或权限修改失败。
 *
 * @warning
 * - 目标函数必须具备 __nocfi noinline 属性。
 * - 如果目标函数被编译器内联，hook 会失败或无效。
 * - 不要 hook 正在执行中的函数，否则可能崩溃。
 * - 同一个函数不能同时安装多个 hook。
 */
struct wuwa_inlinehook* wuwa_install_hook(void* target, void* replace, void** backup);

/**
 * @brief 只恢复目标函数入口，不释放 backup trampoline。
 *
 * 调用方如果可能存在已经进入 hook/backup 的并发任务，应先调用该函数阻止新
 * 调用进入 hook，再等待自己的 in-flight 计数归零，最后调用 `wuwa_free_hook()`。
 *
 * @param hook `wuwa_install_hook` 返回的 hook 结构体指针。
 *
 * @return 成功返回 0，失败返回负 errno。
 */
int wuwa_disable_hook(struct wuwa_inlinehook* hook);

/**
 * @brief 释放已经 disable 的 hook 结构体和 backup trampoline。
 *
 * @param hook `wuwa_install_hook` 返回的 hook 结构体指针。
 */
void wuwa_free_hook(struct wuwa_inlinehook* hook);

/**
 * @brief 移除 inline hook。
 *
 * 恢复目标函数原始入口代码，并释放 hook 结构体。
 *
 * @param hook `wuwa_install_hook` 返回的 hook 结构体指针。
 *
 * @return 成功返回 0，失败返回负 errno。
 *
 * @warning 移除后不要继续使用 backup 函数指针，其内存已经释放。
 */
int wuwa_remove_hook(struct wuwa_inlinehook* hook);

#else  /* !defined(INLINE_HOOK) */

/* 未启用 INLINE_HOOK 时提供空实现，方便调用侧保持同一套初始化流程。 */

#define wuwa_inlinehook_init() ({ \
    0;                             \
})

#define wuwa_inlinehook_cleanup() ({ \
})

#define HOOKABLE_FUNC(ret_type, func_name, ...) \
    ret_type func_name(__VA_ARGS__)

#endif /* INLINE_HOOK */
#endif /* BINDER_TRACE_KMOD_INLINE_HOOK_H */
