#include <linux/errno.h>

#include "bt_common.h"
#include "bt_symbols.h"
#include "bt_utils.h"

struct bt_binder_symbols bt_binder_symbols;

static int bt_resolve_required_symbol(const char *name, unsigned long *address)
{
    *address = kallsyms_lookup_name_cfi_ex(name);
    if (!*address) {
        bt_err("解析必需符号失败: %s\n", name);
        return -ENOENT;
    }

    bt_info("符号 %s 地址: 0x%lx\n", name, *address);
    return 0;
}

static void bt_resolve_optional_symbol(const char *name, unsigned long *address)
{
    *address = kallsyms_lookup_name_cfi_ex(name);
    if (*address) {
        bt_info("符号 %s 地址: 0x%lx\n", name, *address);
    } else {
        bt_warn("可选符号不存在: %s\n", name);
    }
}

int bt_binder_symbols_init(void)
{
    int ret;

    ret = bt_resolve_required_symbol("binder_ioctl", &bt_binder_symbols.binder_ioctl);
    if (ret) {
        return ret;
    }

    ret = bt_resolve_required_symbol(
        "binder_alloc_copy_user_to_buffer",
        &bt_binder_symbols.binder_alloc_copy_user_to_buffer);
    if (ret) {
        return ret;
    }

    bt_resolve_optional_symbol("binder_transaction", &bt_binder_symbols.binder_transaction);

    return 0;
}
