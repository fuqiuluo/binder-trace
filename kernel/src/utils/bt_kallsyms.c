#include "bt_utils.h"

#include <linux/kprobes.h>
#include <linux/string.h>
#include "bt_common.h"

#ifdef CONFIG_CFI_CLANG
#define NO_CFI __nocfi
#else
#define NO_CFI
#endif

typedef unsigned long (*kallsyms_lookup_name_t)(const char* name);
typedef int (*kallsyms_on_each_symbol_t)(
    int (*fn)(void *, const char *, struct module *, unsigned long),
    void *data);

static unsigned long NO_CFI call_kln(kallsyms_lookup_name_t f, const char* n) { return f(n); }

static int NO_CFI call_kallsyms_on_each_symbol(kallsyms_on_each_symbol_t f,
                                               int (*fn)(void *, const char *, struct module *, unsigned long),
                                               void *data)
{
    return f(fn, data);
}

unsigned long kallsyms_lookup_name_ex(const char* name) {
#if LINUX_VERSION_CODE >= KERNEL_VERSION(5, 7, 0)
    static kallsyms_lookup_name_t lookup_name = NULL;
    if (lookup_name == NULL) {
        struct kprobe kp = {.symbol_name = "kallsyms_lookup_name"};

        if (register_kprobe(&kp) < 0) {
            return 0;
        }

        lookup_name = (kallsyms_lookup_name_t)kp.addr;
        unregister_kprobe(&kp);

        if (lookup_name == NULL) {
            wuwa_err("kallsyms_lookup_name 符号不存在\n");
            return 0;
        }
        wuwa_info("kallsyms_lookup_name_ex 位于 %p\n", lookup_name);
    }

    return call_kln(lookup_name, name);
#else
    return kallsyms_lookup_name(name);
#endif
}

struct cfi_symbol_search {
    const char *symbol_name;
    size_t symbol_len;
    unsigned long address;
};

static int match_cfi_symbol(void *data, const char *name, struct module *module, unsigned long address)
{
    struct cfi_symbol_search *search = data;

    if (strncmp(name, search->symbol_name, search->symbol_len) == 0 &&
        name[search->symbol_len] == '$') {
        search->address = address;
        bt_info("匹配 CFI 符号: %s=0x%lx\n", name, address);
        return 1;
    }

    return 0;
}

unsigned long kallsyms_lookup_name_cfi_ex(const char *symbol_name)
{
    unsigned long address;
    kallsyms_on_each_symbol_t on_each_symbol;
    struct cfi_symbol_search search;

    address = kallsyms_lookup_name_ex(symbol_name);
    if (address) {
        return address;
    }

    on_each_symbol = (kallsyms_on_each_symbol_t)kallsyms_lookup_name_ex("kallsyms_on_each_symbol");
    if (!on_each_symbol) {
        bt_err("kallsyms_on_each_symbol 符号不存在，无法扫描 CFI 后缀\n");
        return 0;
    }

    search.symbol_name = symbol_name;
    search.symbol_len = strlen(symbol_name);
    search.address = 0;

    call_kallsyms_on_each_symbol(on_each_symbol, match_cfi_symbol, &search);
    if (!search.address) {
        bt_err("未找到符号: %s 或 %s$*\n", symbol_name, symbol_name);
    }

    return search.address;
}
