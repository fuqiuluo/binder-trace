#include "hijack_arm64.h"

#include "bt_common.h"
#include "bt_utils.h"

#if defined(__aarch64__)

#define BT_HOOK_WRITE_MAX_INSN 16

int (*aarch64_insn_write_ptr)(void*, u32) = NULL;
void (*flush_icache_range_ptr)(unsigned long, unsigned long) = NULL;

#ifdef USE_MULTI_INSN_PATCH
/* 多指令原子 patch 支持。 */
int (*aarch64_insn_patch_text_ptr)(void*[], u32[], int) = NULL;
#endif

int init_arch(void) {
    aarch64_insn_write_ptr = (void*)kallsyms_lookup_name_ex("aarch64_insn_write");
    flush_icache_range_ptr = (void*)kallsyms_lookup_name_ex("caches_clean_inval_pou");
    if (!flush_icache_range_ptr) {
        flush_icache_range_ptr = (void*)kallsyms_lookup_name_ex("__flush_icache_range");
    }

#ifdef USE_MULTI_INSN_PATCH
    aarch64_insn_patch_text_ptr = (void*)kallsyms_lookup_name_ex("aarch64_insn_patch_text");
    if (!aarch64_insn_patch_text_ptr) {
        wuwa_warn("未找到 aarch64_insn_patch_text，回退到单指令 patch\n");
    }
#endif

    return !(aarch64_insn_write_ptr && flush_icache_range_ptr);
}

__nocfi int flush_icache_range_ex(unsigned long start, unsigned long end) {
    if (flush_icache_range_ptr) {
        flush_icache_range_ptr(start, end);
        return 0;
    }
    return -1;
}


__nocfi int hook_write_range(void* target, void* source, int size) {
    int ret = 0, i;
    int insn_cnt = size / INSTRUCTION_SIZE;

#ifdef USE_MULTI_INSN_PATCH
    if (aarch64_insn_patch_text_ptr && insn_cnt > 1) {
        void *addrs[BT_HOOK_WRITE_MAX_INSN];
        u32 insns[BT_HOOK_WRITE_MAX_INSN];

        if (insn_cnt > BT_HOOK_WRITE_MAX_INSN) {
            ret = -EINVAL;
            wuwa_err("patch 指令数量超过上限: %d\n", insn_cnt);
            goto out;
        }

        for (i = 0; i < insn_cnt; i++) {
            addrs[i] = target + (i * INSTRUCTION_SIZE);
            insns[i] = *(u32*)(source + (i * INSTRUCTION_SIZE));
        }

        ret = aarch64_insn_patch_text_ptr(addrs, insns, insn_cnt);

        if (ret) {
            wuwa_err("aarch64_insn_patch_text 失败: %d\n", ret);
            goto out;
        }
        goto flush;
    }
#endif

fallback:
    for (i = 0; i < size; i = i + INSTRUCTION_SIZE) {
        ret = aarch64_insn_write_ptr(target + i, *(u32*)(source + i));
        if (ret) {
            goto out;
        }
    }

flush:
    flush_icache_range_ptr((unsigned long)target, (unsigned long)target + size);

out:
    return ret;
}

static inline bool is_b_instruction(uint32_t inst) { return (inst & ARM64_B_MASK) == ARM64_B_INST; }

static inline bool is_hint_instruction(uint32_t inst) { return (inst & ARM64_HINT_MASK) == ARM64_HINT_INST; }

static inline bool is_security_landing_pad(uint32_t inst) {
    return (inst == ARM64_PACIASP || inst == ARM64_PACIBSP || inst == ARM64_BTI_JC || inst == ARM64_BTI_J);
}

static uint64_t decode_b_target(uint32_t inst, uint64_t pc) {
    if (!is_b_instruction(inst)) {
        return 0;
    }

    /* 提取 26 位有符号立即数偏移。 */
    uint32_t imm26 = BITS32(inst, 25, 0);

    /* 分支偏移为 imm26 << 2，按 4 字节对齐。 */
    uint64_t offset = SIGN64_EXTEND((uint64_t)imm26 << 2u, 28u);

    /* 目标地址 = PC + offset。 */
    return pc + offset;
}

static uint64_t resolve_branch_once(uint64_t addr) {
    uint32_t inst = *(volatile uint32_t*)addr;
    uint64_t target;

    /* 直接无条件跳转。 */
    if (is_b_instruction(inst)) {
        target = decode_b_target(inst, addr);
        if (target) {
            return target;
        }
    }

    if (is_hint_instruction(inst)) {
        uint64_t next_addr = addr + 4;
        uint32_t next_inst = *(volatile uint32_t*)next_addr;

        if (is_b_instruction(next_inst)) {
            target = decode_b_target(next_inst, next_addr);
            if (target) {
                return target;
            }
        }
    }

    /* 不是当前支持识别的跳转模式。 */
    return addr;
}

int resolve_branch_chain(uintptr_t addr, uintptr_t* out_final_addr) {
    const int MAX_CHAIN_DEPTH = 32;
    int depth = 0;

    while (depth < MAX_CHAIN_DEPTH) {
        uintptr_t target = resolve_branch_once(addr);

        if (target == addr) {
            break;
        }

        addr = target;
        depth++;
    }

    *out_final_addr = addr;
    return 0;
}

#endif /* defined(__aarch64__) */
