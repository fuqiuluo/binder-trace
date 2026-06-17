// SPDX-License-Identifier: GPL-2.0-only
#include "bt_utils.h"

#include <asm/tlbflush.h>
#include <linux/mm.h>
#include <linux/vmalloc.h>

#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 12, 0)
static void *__nocfi bt_call_vmalloc_node_range(
    typeof(__vmalloc_node_range) *fn,
    unsigned long size,
    unsigned long align,
    unsigned long start,
    unsigned long end,
    gfp_t gfp_flags,
    pgprot_t pgprot,
    unsigned long vm_flags,
    int node,
    const void *caller)
{
    return fn(size, align, start, end, gfp_flags, pgprot, vm_flags, node, caller);
}
#endif

void *alloc_kernel_exec_memory(size_t size, bool force_rw)
{
    pgprot_t pgprot = PAGE_KERNEL;
    unsigned long vm_flags = VM_FLUSH_RESET_PERMS;
    gfp_t gfp_flags = GFP_KERNEL | __GFP_NOWARN;
    unsigned int align = 1;
    unsigned long start = VMALLOC_START;
    unsigned long end = VMALLOC_END;
    void *p;
    size_t bytes = PAGE_ALIGN(size);

#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 12, 0)
    p = __vmalloc_node_range(bytes, align, start, end, gfp_flags, pgprot,
                             vm_flags, NUMA_NO_NODE,
                             __builtin_return_address(0));
#else
    static typeof(__vmalloc_node_range) *my___vmalloc_node_range;

    if (!my___vmalloc_node_range) {
        my___vmalloc_node_range =
            (typeof(__vmalloc_node_range) *)kallsyms_lookup_name_ex("__vmalloc_node_range");
        if (!my___vmalloc_node_range) {
            bt_err("__vmalloc_node_range 符号不存在\n");
            return NULL;
        }
    }

    p = bt_call_vmalloc_node_range(my___vmalloc_node_range, bytes, align,
                                   start, end, gfp_flags, pgprot, vm_flags,
                                   NUMA_NO_NODE, __builtin_return_address(0));
#endif

    if (!p) {
        bt_err("alloc_kernel_exec_memory 分配失败，大小: %zu\n", bytes);
        return NULL;
    }

    if (force_rw) {
        pte_t *ptep = page_from_virt_kernel((uintptr_t)p);
        pte_t pte;

        if (!ptep) {
            bt_err("获取已分配内存的 PTE 失败，地址: 0x%lx\n",
                   (uintptr_t)p);
            vfree(p);
            return NULL;
        }

        pte = READ_ONCE(*ptep);
        pte = set_pte_bit(pte, __pgprot(PTE_DBM));
        pte = set_pte_bit(pte, __pgprot(PTE_WRITE));
        pte = clear_pte_bit(pte, __pgprot(PTE_RDONLY));
        pte = clear_pte_bit(pte, __pgprot(PTE_PXN));
        pte = clear_pte_bit(pte, __pgprot(PTE_GP));
        pte = set_pte_bit(pte, __pgprot(PTE_UXN));
        set_pte_at_ex(ptep, pte);
        __flush_tlb_kernel_pgtable(p);
    }

    bt_warn("alloc_kernel_exec_memory: 已分配 %zu 字节，地址 %lx\n",
            bytes, (uintptr_t)p);
    return p;
}

void free_kernel_exec_memory(void *addr, size_t size)
{
    vfree(addr);
}
