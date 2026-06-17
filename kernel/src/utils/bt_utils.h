/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef BINDER_TRACE_KMOD_UTILS_H
#define BINDER_TRACE_KMOD_UTILS_H

#include <linux/mm.h>
#include <linux/mm_types.h>
#include <linux/types.h>
#include <linux/version.h>

#include "bt_common.h"

#if LINUX_VERSION_CODE >= KERNEL_VERSION(5, 8, 0)
#include <linux/mmap_lock.h>
#define BT_MM_READ_LOCK(mm) mmap_read_lock(mm)
#define BT_MM_READ_UNLOCK(mm) mmap_read_unlock(mm)
#else
#include <linux/rwsem.h>
#define BT_MM_READ_LOCK(mm) down_read(&(mm)->mmap_sem)
#define BT_MM_READ_UNLOCK(mm) up_read(&(mm)->mmap_sem)
#endif

pte_t *page_from_virt_kernel(uintptr_t addr);
unsigned long kallsyms_lookup_name_ex(const char *symbol_name);
unsigned long kallsyms_lookup_name_cfi_ex(const char *symbol_name);

void *alloc_kernel_exec_memory(size_t size, bool force_rw);
void free_kernel_exec_memory(void *addr, size_t size);
void set_pte_at_ex(pte_t *ptep, pte_t pte);

int cfi_bypass(void);
void show_module(void);
void hide_module(void);

#endif /* BINDER_TRACE_KMOD_UTILS_H */
