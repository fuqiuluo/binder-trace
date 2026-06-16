#ifndef BINDER_TRACE_KMOD_HIJACK_ARM64_H
#define BINDER_TRACE_KMOD_HIJACK_ARM64_H
#if defined(__aarch64__)

#include <linux/types.h>

#define INSTRUCTION_SIZE (4)
#define HOOK_TARGET_OFFSET (0)

/* 安全/CFI 相关 landing pad 指令。 */
#define ARM64_PACIASP           0xD503233F  /* 使用 SP 签名返回地址。 */
#define ARM64_PACIBSP           0xD503237F  /* 使用 SP 和 key B 签名返回地址。 */
#define ARM64_BTI_JC            0xD50324DF  /* 分支目标识别，允许 call/jump。 */
#define ARM64_BTI_J             0xD503249F  /* 分支目标识别，允许 jump。 */

/* 无条件分支指令。 */
#define ARM64_B_INST            0x14000000  /* 无条件跳转 B。 */
#define ARM64_BL_INST           0x94000000  /* 带链接跳转 BL。 */

/* 条件分支指令。 */
#define ARM64_BC_INST           0x54000000  /* 条件跳转 B.cond。 */
#define ARM64_CBZ_INST          0x34000000  /* 比较为 0 时跳转。 */
#define ARM64_CBNZ_INST         0x35000000  /* 比较非 0 时跳转。 */
#define ARM64_TBZ_INST          0x36000000  /* 指定位为 0 时跳转。 */
#define ARM64_TBNZ_INST         0x37000000  /* 指定位非 0 时跳转。 */

/* PC 相对寻址指令。 */
#define ARM64_ADR_INST          0x10000000  /* 取 PC 相对标签地址。 */
#define ARM64_ADRP_INST         0x90000000  /* 取 PC 相对页地址。 */

/* 字面量加载指令。 */
#define ARM64_LDR_LIT_32        0x18000000  /* 加载 32 位寄存器字面量。 */
#define ARM64_LDR_LIT_64        0x58000000  /* 加载 64 位寄存器字面量。 */

#define ARM64_LDRSW_LIT         0x98000000  /* 加载有符号 word 字面量。 */
#define ARM64_PRFM_LIT          0xD8000000  /* 预取内存字面量。 */

/* SIMD 字面量加载指令。 */
#define ARM64_LDR_SIMD_32       0x1C000000  /* 加载 32 位 SIMD&FP 寄存器。 */
#define ARM64_LDR_SIMD_64       0x5C000000  /* 加载 64 位 SIMD&FP 寄存器。 */
#define ARM64_LDR_SIMD_128      0x9C000000  /* 加载 128 位 SIMD&FP 寄存器。 */

/* hint 和特殊指令。 */
#define ARM64_HINT_INST         0xD503201F  /* HINT 指令，例如 NOP。 */

#define ARM64_INST_NOP 0xd503201f
#define ARM64_INST_IGNORE       0x0

/* 指令 mask，用于提取 opcode。 */
#define ARM64_B_MASK            0xFC000000
#define ARM64_BL_MASK           0xFC000000
#define ARM64_BC_MASK           0xFF000010
#define ARM64_CBZ_MASK          0x7F000000
#define ARM64_CBNZ_MASK         0x7F000000
#define ARM64_TBZ_MASK          0x7F000000
#define ARM64_TBNZ_MASK         0x7F000000
#define ARM64_ADR_MASK          0x9F000000
#define ARM64_ADRP_MASK         0x9F000000
#define ARM64_LDR_LIT_32_MASK   0xFF000000
#define ARM64_LDR_LIT_64_MASK   0xFF000000
#define ARM64_LDRSW_LIT_MASK    0xFF000000
#define ARM64_PRFM_LIT_MASK     0xFF000000
#define ARM64_LDR_SIMD_32_MASK  0xFF000000
#define ARM64_LDR_SIMD_64_MASK  0xFF000000
#define ARM64_LDR_SIMD_128_MASK 0xFF000000
#define ARM64_HINT_MASK         0xFFFFF01F
#define ARM64_IGNORE_MASK       0x00000000

#define BITS32(n, high, low) \
    ((uint32_t)((n) << (31u - (high))) >> (31u - (high) + (low)))

#define WBIT(n, pos) \
    (((n) >> (pos)) & 1u)

#define SIGN64_EXTEND(n, len) \
    (((uint64_t)((n) << (63u - ((len) - 1))) >> 63u) ? \
     ((n) | (0xFFFFFFFFFFFFFFFFULL << (len))) : (n))

int init_arch(void);

__nocfi int flush_icache_range_ex(unsigned long start, unsigned long end);

__nocfi int hook_write_range(void *target, void *source, int size);

int resolve_branch_chain(uintptr_t addr, uintptr_t* out_final_addr);

#endif /* defined(__aarch64__) */
#endif /* BINDER_TRACE_KMOD_HIJACK_ARM64_H */
