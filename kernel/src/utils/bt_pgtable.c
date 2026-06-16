#include "bt_utils.h"

#include <asm/pgtable-types.h>
#include <asm/tlbflush.h>
#include <linux/mm.h>
#include <linux/pgtable.h>

typedef void (*bt_sync_icache_dcache_t)(pte_t pteval);
typedef void (*bt_mte_sync_tags_t)(pte_t pte, unsigned int nr_pages);

static void __nocfi bt_call_sync_icache_dcache(bt_sync_icache_dcache_t fn, pte_t pteval)
{
    fn(pteval);
}

static void __nocfi bt_call_mte_sync_tags(bt_mte_sync_tags_t fn, pte_t pte, unsigned int nr_pages)
{
    fn(pte, nr_pages);
}

static struct mm_struct *get_init_mm_safe(void)
{
    static struct mm_struct *mm;

    if (unlikely(!mm)) {
        void *sym = (void *)kallsyms_lookup_name_ex("init_mm");

        if (!sym)
            return NULL;
        mm = (struct mm_struct *)sym;
    }

    return mm;
}

pte_t *page_from_virt_kernel(uintptr_t addr)
{
    struct mm_struct *mm = get_init_mm_safe();
    pgd_t *pgd;
    p4d_t *p4d;
    pud_t *pud;
    pmd_t *pmd;
    pte_t *pte;

    if (!mm)
        return NULL;

    pgd = pgd_offset(mm, addr);
    if (pgd_none(*pgd) || pgd_bad(*pgd))
        return NULL;

    p4d = p4d_offset(pgd, addr);
    if (p4d_none(*p4d) || p4d_bad(*p4d))
        return NULL;

    pud = pud_offset(p4d, addr);
#if defined(pud_sect)
    if (pud_sect(*pud))
        return (pte_t *)pud;
#endif
    if (pud_none(*pud) || pud_bad(*pud))
        return NULL;

    pmd = pmd_offset(pud, addr);
#if defined(pmd_sect)
    if (pmd_sect(*pmd))
        return (pte_t *)pmd;
#endif
    if (pmd_none(*pmd) || pmd_bad(*pmd))
        return NULL;

    pte = pte_offset_kernel(pmd, addr);
    if (!pte || !pte_present(*pte))
        return NULL;

    return pte;
}

void set_pte_at_ex(pte_t *ptep, pte_t pte)
{
    static bt_sync_icache_dcache_t my__sync_icache_dcache;
    static bt_mte_sync_tags_t my_mte_sync_tags;

    if (!my__sync_icache_dcache)
        my__sync_icache_dcache =
            (bt_sync_icache_dcache_t)kallsyms_lookup_name_ex("__sync_icache_dcache");

    if (!my_mte_sync_tags)
        my_mte_sync_tags =
            (bt_mte_sync_tags_t)kallsyms_lookup_name_ex("mte_sync_tags");

    if (pte_present(pte) && pte_user_exec(pte) && !pte_special(pte)) {
        if (my__sync_icache_dcache)
            bt_call_sync_icache_dcache(my__sync_icache_dcache, pte);
        else
            bt_warn("__sync_icache_dcache 符号不存在，跳过 icache 同步\n");
    }

    if (system_supports_mte() && pte_access_permitted(pte, false) &&
        !pte_special(pte) && pte_tagged(pte)) {
        if (my_mte_sync_tags)
            bt_call_mte_sync_tags(my_mte_sync_tags, pte, 1);
    }

    WRITE_ONCE(*ptep, pte);
    dsb(ishst);
    isb();
}
