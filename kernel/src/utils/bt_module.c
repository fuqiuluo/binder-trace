// SPDX-License-Identifier: GPL-2.0-only
#include "bt_utils.h"

#include "bt_common.h"
#include "hijack_arm64.h"
#include <linux/list.h>
#include <linux/module.h>
#include <linux/proc_fs.h>
#include <linux/string.h>

/*
 * 开源发布暂时禁用模块隐藏实现。代码保留在仓库中供审计，但不参与编译。
 */
#if 0
static struct list_head *module_previous;
static struct kobject *module_kobj_parent;
static struct kset *module_kobj_kset;
static char module_name_saved[MODULE_NAME_LEN];
static short module_hidden = 0;

void show_module(void) {
    if (!module_hidden)
        return;

    memcpy(THIS_MODULE->name, module_name_saved, MODULE_NAME_LEN);

    list_add(&THIS_MODULE->list, module_previous);

    /* Restore kset before kobject_add so kobj_kset_join re-links kobj->entry */
    THIS_MODULE->mkobj.kobj.kset = module_kobj_kset;
    if (kobject_add(&THIS_MODULE->mkobj.kobj, module_kobj_parent, "%s", THIS_MODULE->name))
        wuwa_err("show_module: kobject_add failed\n");

    module_hidden = 0;
}

void hide_module(void) {
    if (module_hidden)
        return;

    remove_proc_entry("sched_debug", NULL);
    remove_proc_entry("uevents_records", NULL);

#ifdef MODULE
    /* Save before kobject_del clears parent/kset, and before list_del */
    module_previous = THIS_MODULE->list.prev;
    module_kobj_parent = THIS_MODULE->mkobj.kobj.parent;
    module_kobj_kset = THIS_MODULE->mkobj.kobj.kset;
    memcpy(module_name_saved, THIS_MODULE->name, MODULE_NAME_LEN);

    list_del(&THIS_MODULE->list);
    /* kobject_del calls kobj_kset_leave which does list_del_init on kobj->entry
     * internally — do NOT call list_del on entry afterwards or it leaves poison
     * pointers that break kobject_add on show. */
    kobject_del(&THIS_MODULE->mkobj.kobj);
#endif

    memcpy(THIS_MODULE->name, "nfc\0", 4);
    module_hidden = 1;
}
#endif


int cfi_bypass(void) {
    int ret = 0;
    unsigned int RET = 0xD65F03C0; // ret (aarch64)
    unsigned int MOV_X0_1 = 0xD2800020; // mov x0, #1

    unsigned long f__cfi_slowpath = kallsyms_lookup_name_ex("__cfi_slowpath");
    if (f__cfi_slowpath) {
        unsigned int* p = (unsigned int*)f__cfi_slowpath;
        if (*p != RET) {
            hook_write_range(p, &RET, INSTRUCTION_SIZE);
            ret++;
            wuwa_err("patch __cfi_slowpath successed\n");
        } else {
            wuwa_info("__cfi_slowpath already patched\n");
        }
    }

    unsigned long f__cfi_slowpath_diag = kallsyms_lookup_name_ex("__cfi_slowpath_diag");
    if (f__cfi_slowpath_diag) {
        unsigned int* p = (unsigned int*)f__cfi_slowpath_diag;
        if (*p != RET) {
            hook_write_range(p, &RET, INSTRUCTION_SIZE);
            ret++;
            wuwa_err("patch __cfi_slowpath_diag successed\n");
        } else {
            wuwa_info("__cfi_slowpath_diag already patched\n");
        }
    }

    unsigned long f_cfi_slowpath = kallsyms_lookup_name_ex("_cfi_slowpath");
    if (f_cfi_slowpath) {
        unsigned int* p = (unsigned int*)f_cfi_slowpath;
        if (*p != RET) {
            hook_write_range(p, &RET, INSTRUCTION_SIZE);
            ret++;
            wuwa_err("patch _cfi_slowpath successed\n");
        } else {
            wuwa_info("_cfi_slowpath already patched\n");
        }
    }

    unsigned long f__cfi_check_fail = kallsyms_lookup_name_ex("__cfi_check_fail");
    if (f__cfi_check_fail) {
        unsigned int* p = (unsigned int*)f__cfi_check_fail;
        if (*p != RET) {
            hook_write_range(p, &RET, INSTRUCTION_SIZE);
            ret++;
            wuwa_err("patch __cfi_check_fail successed\n");
        } else {
            wuwa_info("__cfi_check_fail already patched\n");
        }
    }

    unsigned long f__ubsan_handle_cfi_check_fail_abort = kallsyms_lookup_name_ex("__ubsan_handle_cfi_check_fail_abort");
    if (f__ubsan_handle_cfi_check_fail_abort) {
        unsigned int* p = (unsigned int*)f__ubsan_handle_cfi_check_fail_abort;
        if (*p != RET) {
            hook_write_range(p, &RET, INSTRUCTION_SIZE);
            ret++;
            wuwa_err("patch __ubsan_handle_cfi_check_fail_abort successed\n");
        } else {
            wuwa_info("__ubsan_handle_cfi_check_fail_abort already patched\n");
        }
    }

    unsigned long f__ubsan_handle_cfi_check_fail = kallsyms_lookup_name_ex("__ubsan_handle_cfi_check_fail");
    if (f__ubsan_handle_cfi_check_fail) {
        unsigned int* p = (unsigned int*)f__ubsan_handle_cfi_check_fail;
        if (*p != RET) {
            hook_write_range(p, &RET, INSTRUCTION_SIZE);
            ret++;
            wuwa_err("patch __ubsan_handle_cfi_check_fail successed\n");
        } else {
            wuwa_info("__ubsan_handle_cfi_check_fail already patched\n");
        }
    }

    unsigned long freport_cfi_failure = kallsyms_lookup_name_ex("report_cfi_failure");
    if (freport_cfi_failure) {
        unsigned int* p = (unsigned int*)freport_cfi_failure;
        if (*p != MOV_X0_1) {
            hook_write_range(p, &MOV_X0_1, INSTRUCTION_SIZE);
            hook_write_range(p + 1, &RET, INSTRUCTION_SIZE);
            ret++;
        } else {
            wuwa_info("report_cfi_failure already patched\n");
        }
    }

    return ret;
}
