// SPDX-License-Identifier: GPL-2.0-only
#include "inline_hook.h"
#include "hijack_arm64.h"
#include "linux/compiler_attributes.h"
#include "linux/types.h"
#include "bt_common.h"
#include "bt_utils.h"

#include <asm/pgtable.h>
#include <asm/tlbflush.h>
#include <linux/errno.h>
#include <linux/mm.h>
#include <linux/rcupdate.h>

#if defined(INLINE_HOOK)

// ARM64 NOP 指令别名。
#define ARM64_NOP ARM64_INST_NOP
#define ARM64_BR_X17 0xd61f0220
#define ARM64_RET_X17 0xd65f0220

// 需要重定位处理的 ARM64 指令类型。
typedef enum {
    INST_B = ARM64_B_INST,
    INST_BC = ARM64_BC_INST,
    INST_BL = ARM64_BL_INST,
    INST_ADR = ARM64_ADR_INST,
    INST_ADRP = ARM64_ADRP_INST,
    INST_LDR_32 = ARM64_LDR_LIT_32,
    INST_LDR_64 = ARM64_LDR_LIT_64,
    INST_LDRSW_LIT = ARM64_LDRSW_LIT,
    INST_PRFM_LIT = ARM64_PRFM_LIT,
    INST_LDR_SIMD_32 = ARM64_LDR_SIMD_32,
    INST_LDR_SIMD_64 = ARM64_LDR_SIMD_64,
    INST_LDR_SIMD_128 = ARM64_LDR_SIMD_128,
    INST_CBZ = ARM64_CBZ_INST,
    INST_CBNZ = ARM64_CBNZ_INST,
    INST_TBZ = ARM64_TBZ_INST,
    INST_TBNZ = ARM64_TBNZ_INST,
} inst_type_t;

// 用于识别和重定位指令的 opcode mask/type 表。
static uint32_t masks[] = {
    ARM64_B_MASK,           ARM64_BC_MASK,           ARM64_BL_MASK,        ARM64_ADR_MASK,      ARM64_ADRP_MASK,
    ARM64_LDR_LIT_32_MASK,  ARM64_LDR_LIT_64_MASK,   ARM64_LDRSW_LIT_MASK, ARM64_PRFM_LIT_MASK, ARM64_LDR_SIMD_32_MASK,
    ARM64_LDR_SIMD_64_MASK, ARM64_LDR_SIMD_128_MASK, ARM64_CBZ_MASK,       ARM64_CBNZ_MASK,     ARM64_TBZ_MASK,
    ARM64_TBNZ_MASK,        ARM64_IGNORE_MASK,
};

static uint32_t types[] = {
    ARM64_B_INST,     ARM64_BC_INST,   ARM64_BL_INST,  ARM64_ADR_INST,    ARM64_ADRP_INST,   ARM64_LDR_LIT_32,
    ARM64_LDR_LIT_64, ARM64_LDRSW_LIT, ARM64_PRFM_LIT, ARM64_LDR_SIMD_32, ARM64_LDR_SIMD_64, ARM64_LDR_SIMD_128,
    ARM64_CBZ_INST,   ARM64_CBNZ_INST, ARM64_TBZ_INST, ARM64_TBNZ_INST,   ARM64_INST_IGNORE,
};

static int32_t relo_len[] = {6, 8, 6, 4, 4, 6, 6, 6, 8, 8, 8, 8, 6, 6, 6, 6, 2};

static bool is_in_backup_code(struct wuwa_inlinehook* hook, uintptr_t addr) {
    uintptr_t start = hook->addr.backup_addr;
    uintptr_t end = start + sizeof(hook->insn.relocated_insns);

    return addr >= start && addr < end;
}

static uint32_t branch_back_x17_insn(void) {
    if (bt_preserve_bti) {
        return ARM64_RET_X17;
    }

    /*
     * 保守写法原本是 RET X17:
     * return ARM64_RET_X17;
     *
     * 原因是 backup trampoline 跳回的地址通常是 original+N，也就是函数
     * 内部普通指令，不是 BTI landing pad。BR X17 会按目标页的 PTE_GP 做
     * BTI 检查，目标点没有 BTI/PAC landing pad 时会触发异常；RET X17 不
     * 要求返回目标是 BTI landing pad。默认策略改用 BR X17，因此安装 hook
     * 时会临时清理相关 text 页的 PTE_GP，卸载时再恢复。
     */
    return ARM64_BR_X17;
}

static int hook_track_bti_guard_page(struct wuwa_inlinehook* hook, uintptr_t addr) {
    uintptr_t page_addr = addr & PAGE_MASK;
    pte_t *ptep;
    pte_t pte;
    int i;

    if (bt_preserve_bti || is_in_backup_code(hook, addr)) {
        return 0;
    }

    for (i = 0; i < BTI_GUARD_PAGE_NUM; i++) {
        if (hook->bti_guard_pages[i].active &&
            hook->bti_guard_pages[i].page_addr == page_addr) {
            return 0;
        }
    }

    for (i = 0; i < BTI_GUARD_PAGE_NUM; i++) {
        if (!hook->bti_guard_pages[i].active) {
            break;
        }
    }
    if (i == BTI_GUARD_PAGE_NUM) {
        wuwa_err("BTI guard 页记录槽已满，target=0x%lx\n", addr);
        return -ENOSPC;
    }

    ptep = page_from_virt_kernel(page_addr);
    if (!ptep) {
        wuwa_err("获取 hooked text 页 PTE 失败，target=0x%lx page=0x%lx\n",
                 addr, page_addr);
        return -EACCES;
    }

    pte = READ_ONCE(*ptep);
    hook->bti_guard_pages[i].active = true;
    hook->bti_guard_pages[i].page_addr = page_addr;
    hook->bti_guard_pages[i].ptep = ptep;
    hook->bti_guard_pages[i].had_gp = !!(pte_val(pte) & PTE_GP);

    if (!hook->bti_guard_pages[i].had_gp) {
        return 0;
    }

    pte = clear_pte_bit(pte, __pgprot(PTE_GP));
    set_pte_at_ex(ptep, pte);
    __flush_tlb_kernel_pgtable((void*)page_addr);
    dsb(ish);
    isb();

    wuwa_info("已清理 hooked text 页 PTE_GP: page=0x%lx target=0x%lx\n",
              page_addr, addr);
    return 0;
}

static void hook_restore_bti_guard_pages(struct wuwa_inlinehook* hook) {
    int i;

    for (i = 0; i < BTI_GUARD_PAGE_NUM; i++) {
        struct hook_bti_guard_page *guard = &hook->bti_guard_pages[i];
        pte_t *ptep;
        pte_t pte;

        if (!guard->active) {
            continue;
        }

        ptep = (pte_t*)guard->ptep;
        if (ptep && guard->had_gp) {
            pte = READ_ONCE(*ptep);
            pte = set_pte_bit(pte, __pgprot(PTE_GP));
            set_pte_at_ex(ptep, pte);
            __flush_tlb_kernel_pgtable((void*)guard->page_addr);
            dsb(ish);
            isb();
            wuwa_info("已恢复 hooked text 页 PTE_GP: page=0x%lx\n",
                      guard->page_addr);
        }

        guard->active = false;
        guard->ptep = NULL;
        guard->page_addr = 0;
        guard->had_gp = false;
    }
}

// 判断地址是否落在目标函数入口 trampoline 范围内。
static int is_in_tramp(struct wuwa_inlinehook* hook, uint64_t addr) {
    uint64_t tramp_start = hook->addr.resolved_addr;
    uint64_t tramp_end = tramp_start + hook->insn.trampoline_count * INSTRUCTION_SIZE;
    return (addr >= tramp_start && addr < tramp_end);
}

// 如果地址落在 trampoline 范围内，换算到重定位后的 backup 地址。
static uint64_t relo_in_tramp(struct wuwa_inlinehook* hook, uint64_t addr) {
    int i, j;
    uintptr_t tramp_start = hook->addr.resolved_addr;
    uintptr_t tramp_end = tramp_start + hook->insn.trampoline_count * INSTRUCTION_SIZE;
    uint32_t addr_inst_index = (addr - tramp_start) / INSTRUCTION_SIZE;
    uintptr_t fix_addr = hook->addr.backup_addr;

    if (!(addr >= tramp_start && addr < tramp_end))
        return addr;

    // 根据前序指令重定位后的长度累计计算新地址。
    for (i = 0; i < addr_inst_index; i++) {
        uint32_t inst = hook->insn.saved_insns[i];
        for (j = 0; j < sizeof(relo_len) / sizeof(relo_len[0]); j++) {
            if ((inst & masks[j]) == types[j]) {
                fix_addr += relo_len[j] * INSTRUCTION_SIZE;
                break;
            }
        }
    }
    return fix_addr;
}


__nocfi int wuwa_inlinehook_init(void) {
    return 0;
}

__nocfi void wuwa_inlinehook_cleanup(void) {
}

static int branch_func_addr_once(uintptr_t addr, uintptr_t* out_final_addr) {
    *out_final_addr = resolve_branch_once(addr);
    return 0;
}

int branch_absolute_addr(uint32_t* buf, uintptr_t target) {
    buf[0] = 0x58000051; // LDR X17, #8，加载绝对跳转目标。
    buf[1] = ARM64_BR_X17; // BR X17，跳转到目标地址。
    buf[2] = target & 0xFFFFFFFF;
    buf[3] = target >> 32u;
    return 4;
}

static int branch_absolute_addr_as_return(struct wuwa_inlinehook* hook, uint32_t* buf, uintptr_t target) {
    int ret = hook_track_bti_guard_page(hook, target);
    if (ret) {
        return ret;
    }

    buf[0] = 0x58000051; // LDR X17, #8，加载续执行地址。
    buf[1] = branch_back_x17_insn(); // 默认 BR X17；preserve_bti=1 时使用 RET X17 保守策略。
    buf[2] = target & 0xFFFFFFFF;
    buf[3] = target >> 32u;
    return 4;
}


int relo_b(struct wuwa_inlinehook* hook, uint64_t inst_addr, uint32_t inst, inst_type_t type) {
    uint32_t* buf = hook->insn.relocated_insns + hook->insn.relocated_count;
    uint64_t imm64, addr;
    uint32_t idx = 0;
    int ret;

    if (type == INST_BC) {
        uint64_t imm19 = BITS32(inst, 23, 5);
        imm64 = SIGN64_EXTEND(imm19 << 2u, 21u);
    } else {
        uint64_t imm26 = BITS32(inst, 25, 0);
        imm64 = SIGN64_EXTEND(imm26 << 2u, 28u);
    }
    addr = inst_addr + imm64;
    addr = relo_in_tramp(hook, addr);

    if (type == INST_BC) {
        buf[idx++] = (inst & 0xFF00001F) | 0x40u; // B.<cond> #8，保留条件分支语义。
        buf[idx++] = 0x14000006; // B #24，跳过绝对跳转序列。
    }
    buf[idx++] = 0x58000051; // LDR X17, #8，加载分支目标。
    buf[idx++] = 0x14000003; // B #12，跳过内嵌地址字面量。
    buf[idx++] = addr & 0xFFFFFFFF;
    buf[idx++] = addr >> 32u;
    if (type == INST_BL) {
        buf[idx++] = 0xD63F0220; // BLR X17，保留带链接调用。
    } else {
        ret = hook_track_bti_guard_page(hook, addr);
        if (ret) {
            return ret;
        }
        buf[idx++] = branch_back_x17_insn(); // 默认 BR X17；preserve_bti=1 时使用 RET X17 保守策略。
    }
    buf[idx++] = ARM64_NOP;
    return 0;
}

int relo_adr(struct wuwa_inlinehook* hook, uint64_t inst_addr, uint32_t inst, inst_type_t type) {
    uint32_t* buf = hook->insn.relocated_insns + hook->insn.relocated_count;

    uint32_t xd = BITS32(inst, 4, 0);
    uint64_t immlo = BITS32(inst, 30, 29);
    uint64_t immhi = BITS32(inst, 23, 5);
    uint64_t addr;

    if (type == INST_ADR) {
        addr = inst_addr + SIGN64_EXTEND((immhi << 2u) | immlo, 21u);
    } else {
        addr = (inst_addr + SIGN64_EXTEND((immhi << 14u) | (immlo << 12u), 33u)) & 0xFFFFFFFFFFFFF000;
        if (is_in_tramp(hook, addr))
            return -EOPNOTSUPP;
    }
    buf[0] = 0x58000040u | xd; // LDR Xd, #8，加载重定位后的地址。
    buf[1] = 0x14000003; // B #12，跳过内嵌地址字面量。
    buf[2] = addr & 0xFFFFFFFF;
    buf[3] = addr >> 32u;
    return 0;
}

int relo_ldr(struct wuwa_inlinehook* hook, uint64_t inst_addr, uint32_t inst, inst_type_t type) {
    uint32_t* buf = hook->insn.relocated_insns + hook->insn.relocated_count;

    uint32_t rt = BITS32(inst, 4, 0);
    uint64_t imm19 = BITS32(inst, 23, 5);
    uint64_t offset = SIGN64_EXTEND((imm19 << 2u), 21u);
    uint64_t addr = inst_addr + offset;

    if (is_in_tramp(hook, addr) && type != INST_PRFM_LIT)
        return -EOPNOTSUPP;

    addr = relo_in_tramp(hook, addr);

    if (type == INST_LDR_32 || type == INST_LDR_64 || type == INST_LDRSW_LIT) {
        buf[0] = 0x58000060u | rt; // LDR Xt, #12，加载字面量地址。
        if (type == INST_LDR_32) {
            buf[1] = 0xB9400000 | rt | (rt << 5u); // LDR Wt, [Xt]，读取 32 位值。
        } else if (type == INST_LDR_64) {
            buf[1] = 0xF9400000 | rt | (rt << 5u); // LDR Xt, [Xt]，读取 64 位值。
        } else {
            // LDRSW literal，读取有符号 32 位值并扩展到 64 位。
            buf[1] = 0xB9800000 | rt | (rt << 5u); // LDRSW Xt, [Xt]。
        }
        buf[2] = 0x14000004; // B #16，跳过地址字面量。
        buf[3] = ARM64_NOP;
        buf[4] = addr & 0xFFFFFFFF;
        buf[5] = addr >> 32u;
    } else {
        buf[0] = 0xA93F47F0; // STP X16, X17, [SP, -0x10]，临时保存寄存器。
        buf[1] = 0x58000091; // LDR X17, #16，加载目标地址。
        if (type == INST_PRFM_LIT) {
            buf[2] = 0xF9800220 | rt; // PRFM Rt, [X17]，重放预取。
        } else if (type == INST_LDR_SIMD_32) {
            buf[2] = 0xBD400220 | rt; // LDR St, [X17]，读取 SIMD 32 位值。
        } else if (type == INST_LDR_SIMD_64) {
            buf[2] = 0xFD400220 | rt; // LDR Dt, [X17]，读取 SIMD 64 位值。
        } else {
            // LDR_SIMD_128，读取 SIMD 128 位值。
            buf[2] = 0x3DC00220u | rt; // LDR Qt, [X17]。
        }
        buf[3] = 0xF85F83F1; // LDR X17, [SP, -0x8]，恢复 X17。
        buf[4] = 0x14000004; // B #16，跳过地址字面量。
        buf[5] = ARM64_NOP;
        buf[6] = addr & 0xFFFFFFFF;
        buf[7] = addr >> 32u;
    }
    return 0;
}

int relo_cb(struct wuwa_inlinehook* hook, uint64_t inst_addr, uint32_t inst, inst_type_t type) {
    uint32_t* buf = hook->insn.relocated_insns + hook->insn.relocated_count;
    int ret;

    uint64_t imm19 = BITS32(inst, 23, 5);
    uint64_t offset = SIGN64_EXTEND((imm19 << 2u), 21u);
    uint64_t addr = inst_addr + offset;
    addr = relo_in_tramp(hook, addr);
    ret = hook_track_bti_guard_page(hook, addr);
    if (ret) {
        return ret;
    }

    buf[0] = (inst & 0xFF00001F) | 0x40u; // CB(N)Z Rt, #8，保留比较分支。
    buf[1] = 0x14000005; // B #20，跳过绝对跳转序列。
    buf[2] = 0x58000051; // LDR X17, #8，加载分支目标。
    buf[3] = branch_back_x17_insn(); // 默认 BR X17；preserve_bti=1 时使用 RET X17 保守策略。
    buf[4] = addr & 0xFFFFFFFF;
    buf[5] = addr >> 32u;
    return 0;
}

int relo_tb(struct wuwa_inlinehook* hook, uint64_t inst_addr, uint32_t inst, inst_type_t type) {
    uint32_t* buf = hook->insn.relocated_insns + hook->insn.relocated_count;
    int ret;

    uint64_t imm14 = BITS32(inst, 18, 5);
    uint64_t offset = SIGN64_EXTEND((imm14 << 2u), 16u);
    uint64_t addr = inst_addr + offset;
    addr = relo_in_tramp(hook, addr);
    ret = hook_track_bti_guard_page(hook, addr);
    if (ret) {
        return ret;
    }

    buf[0] = (inst & 0xFFF8001F) | 0x40u; // TB(N)Z Rt, #<imm>, #8，保留位测试分支。
    buf[1] = 0x14000005; // B #20，跳过绝对跳转序列。
    buf[2] = 0x58000051; // LDR X17, #8，加载分支目标。
    buf[3] = branch_back_x17_insn(); // 默认 BR X17；preserve_bti=1 时使用 RET X17 保守策略。
    buf[4] = addr & 0xFFFFFFFF;
    buf[5] = addr >> 32u;
    return 0;
}

int relo_ignore(struct wuwa_inlinehook* hook, uint64_t inst_addr, uint32_t inst, inst_type_t type) {
    uint32_t* buf = hook->insn.relocated_insns + hook->insn.relocated_count;
    buf[0] = inst;
    buf[1] = ARM64_NOP;
    return 0;
}

static int relocate_inst(struct wuwa_inlinehook* hook, uintptr_t inst_addr, uint32_t inst) {
    int ret = 0;
    uint32_t it = ARM64_INST_IGNORE;
    int len = 1, j = 0;

    for (j = 0; j < sizeof(relo_len) / sizeof(relo_len[0]); j++) {
        if ((inst & masks[j]) == types[j]) {
            it = types[j];
            len = relo_len[j];
            break;
        }
    }

    switch (it) {
    case ARM64_B_INST:
    case ARM64_BC_INST:
    case ARM64_BL_INST:
        ret = relo_b(hook, inst_addr, inst, it);
        break;
    case ARM64_ADR_INST:
    case ARM64_ADRP_INST:
        ret = relo_adr(hook, inst_addr, inst, it);
        break;
    case ARM64_LDR_LIT_32:
    case ARM64_LDR_LIT_64:
    case ARM64_LDRSW_LIT:
    case ARM64_PRFM_LIT:
    case ARM64_LDR_SIMD_32:
    case ARM64_LDR_SIMD_64:
    case ARM64_LDR_SIMD_128:
        ret = relo_ldr(hook, inst_addr, inst, it);
        break;
    case ARM64_CBZ_INST:
    case ARM64_CBNZ_INST:
        ret = relo_cb(hook, inst_addr, inst, it);
        break;
    case ARM64_TBZ_INST:
    case ARM64_TBNZ_INST:
        ret = relo_tb(hook, inst_addr, inst, it);
        break;
    case ARM64_INST_IGNORE:
    default:
        ret = relo_ignore(hook, inst_addr, inst, it);
        break;
    }

    if (ret < 0) {
        return ret;
    }

    hook->insn.relocated_count += len;

    return 0;
}

__nocfi struct wuwa_inlinehook* wuwa_install_hook(void* target, void* replace, void** backup) {
    if (!target || !replace || !backup) {
        wuwa_err("参数非法: target=%px, replace=%px, backup=%px\n",
                 target, replace, backup);
        return ERR_PTR(-EINVAL);
    }

    int ret = 0, i;
    uintptr_t back_dst_addr;
    uintptr_t final_target = (uintptr_t)target;
    uint32_t* buf;

    // 解析入口跳转链，找到实际可 patch 的函数地址。
    ret = resolve_branch_chain((uintptr_t)target, &final_target);
    if (ret) {
        wuwa_err("解析 %px 的跳转链失败: %d\n", target, ret);
        return ERR_PTR(ret);
    }

    // 分配可执行内存，hook 结构体内含 backup 指令序列。
    struct wuwa_inlinehook* hook =
        (struct wuwa_inlinehook*)alloc_kernel_exec_memory(sizeof(struct wuwa_inlinehook), true);
    if (!hook) {
        wuwa_err("为 hook 结构体分配内存失败\n");
        return ERR_PTR(-ENOMEM);
    }
    memset(hook, 0, sizeof(*hook));

    // 初始化地址信息。
    hook->addr.target_func = (uintptr_t)target;
    hook->addr.replacement_func = (uintptr_t)replace;
    hook->addr.resolved_addr = final_target;
    hook->addr.backup_addr = (uintptr_t)(hook->insn.relocated_insns);
    *backup = (void*)(hook->addr.backup_addr);

    /*
     * 默认 trampoline 第一条是 LDR，不再是原函数入口的 BTI/PAC landing pad。
     * 清掉入口页 PTE_GP 后，间接调用落到 hooked 函数入口也不会触发 BTI。
     */
    ret = hook_track_bti_guard_page(hook, hook->addr.resolved_addr);
    if (ret) {
        goto err_free_hook;
    }

    // 保存原始入口指令，卸载 hook 时用于恢复。
    for (i = 0; i < TRAMPOLINE_NUM; i++) {
        hook->insn.saved_insns[i] = *((uint32_t*)hook->addr.resolved_addr + i);
    }

    // 生成跳转到替换函数的 trampoline 指令。
    hook->insn.trampoline_count = branch_absolute_addr(hook->insn.trampoline_insns, hook->addr.replacement_func);

    // 初始化重定位指令缓冲区。
    for (i = 0; i < sizeof(hook->insn.relocated_insns) / sizeof(hook->insn.relocated_insns[0]); i++) {
        hook->insn.relocated_insns[i] = ARM64_INST_NOP;
    }

    // 重定位原始入口指令，供 backup 函数执行。
    for (i = 0; i < hook->insn.trampoline_count; i++) {
        uint64_t inst_addr = hook->addr.resolved_addr + i * INSTRUCTION_SIZE;
        uint32_t inst = hook->insn.saved_insns[i];
        int relo_res = relocate_inst(hook, inst_addr, inst);
        if (relo_res < 0) {
            wuwa_err("重定位入口指令失败: addr=0x%llx inst=0x%x ret=%d\n",
                     (unsigned long long)inst_addr, inst, relo_res);
            ret = relo_res;
            goto err_free_hook;
        }
    }

    // 在重定位代码末尾追加跳回原函数剩余部分的分支。
    back_dst_addr = hook->addr.resolved_addr + hook->insn.trampoline_count * INSTRUCTION_SIZE;
    buf = hook->insn.relocated_insns + hook->insn.relocated_count;
    ret = branch_absolute_addr_as_return(hook, buf, back_dst_addr);
    if (ret < 0) {
        goto err_free_hook;
    }
    hook->insn.relocated_count += ret;

    // 刷新重定位代码的指令缓存。
    flush_icache_range_ex(hook->addr.backup_addr,
                         hook->addr.backup_addr + hook->insn.relocated_count * INSTRUCTION_SIZE);
    dsb(ish);
    isb();

    // 把 trampoline 写入目标函数入口。
    ret = hook_write_range((void*)hook->addr.resolved_addr,
                           hook->insn.trampoline_insns,
                           hook->insn.trampoline_count * INSTRUCTION_SIZE);
    if (ret) {
        wuwa_err("安装 trampoline 失败: %d\n", ret);
        goto err_free_hook;
    }

    return hook;

err_free_hook:
    hook_restore_bti_guard_pages(hook);
    free_kernel_exec_memory(hook, sizeof(struct wuwa_inlinehook));
    return ERR_PTR(ret);
}

__nocfi int wuwa_disable_hook(struct wuwa_inlinehook* hook) {
    if (!hook) {
        wuwa_err("hook 参数非法: NULL\n");
        return -EINVAL;
    }

    int ret = 0;
    uintptr_t origin = hook->addr.target_func;

    if (hook->disabled) {
        return 0;
    }

    // 重新解析跳转链，确认实际恢复地址。
    ret = resolve_branch_chain(hook->addr.target_func, &origin);
    if (ret) {
        wuwa_err("解析跳转链失败: %d\n", ret);
        return ret;
    }

    // 恢复原始入口指令。
    ret = hook_write_range((void*)hook->addr.resolved_addr,
                           hook->insn.saved_insns,
                           hook->insn.trampoline_count * INSTRUCTION_SIZE);
    if (ret) {
        wuwa_err("恢复原始指令失败: %d\n", ret);
        return ret;
    }

    hook->disabled = true;
    return ret;
}

__nocfi int wuwa_free_hook(struct wuwa_inlinehook* hook) {
    if (!hook) {
        return 0;
    }

    if (!hook->disabled) {
        wuwa_err("拒绝释放仍启用的 hook: target=0x%lx backup=0x%lx\n",
                 hook->addr.resolved_addr,
                 hook->addr.backup_addr);
        return -EBUSY;
    }

    hook_restore_bti_guard_pages(hook);
    free_kernel_exec_memory(hook, sizeof(struct wuwa_inlinehook));
    return 0;
}

__nocfi int wuwa_remove_hook(struct wuwa_inlinehook* hook) {
    int ret = wuwa_disable_hook(hook);

    if (ret) {
        return ret;
    }

    /*
     * 简单调用方没有自己的 in-flight 计数，只能用 Tasks RCU 等待已经进入
     * 旧入口/trampoline 的任务离开代码修改窗口。
     */
    synchronize_rcu_tasks();
    return wuwa_free_hook(hook);
}

#endif /* INLINE_HOOK */
