#include "bt_control.h"

#include <linux/errno.h>
#include <linux/uaccess.h>

#include "bt_capture.h"
#include "bt_common.h"
#include "bt_ipc_uapi.h"

long bt_control_dispatch(unsigned int cmd, unsigned long arg)
{
    void __user *argp = (void __user *)arg;
    struct bt_capture_config config;
    struct bt_capture_stats stats;
    struct bt_abi_version version;
    struct bt_driver_feature feature;

    if (_IOC_TYPE(cmd) != BT_IOC_MAGIC) {
        bt_warn("控制命令 magic 无效: 0x%x\n", _IOC_TYPE(cmd));
        return -ENOTTY;
    }

    if (_IOC_NR(cmd) > BT_IOC_MAX_NR) {
        bt_warn("控制命令编号越界: %u\n", _IOC_NR(cmd));
        return -ENOTTY;
    }

    switch (cmd) {
    case BT_IOC_GET_ABI_VERSION:
        version.version = BT_ABI_VERSION;
        version._reserved = 0;
        if (copy_to_user(argp, &version, sizeof(version))) {
            return -EFAULT;
        }
        return 0;

    case BT_IOC_SET_CONFIG:
        if (copy_from_user(&config, argp, sizeof(config))) {
            return -EFAULT;
        }
        return bt_capture_set_config(&config);

    case BT_IOC_GET_CONFIG:
        bt_capture_get_config(&config);
        if (copy_to_user(argp, &config, sizeof(config))) {
            return -EFAULT;
        }
        return 0;

    case BT_IOC_GET_STATS:
        bt_capture_get_stats(&stats);
        if (copy_to_user(argp, &stats, sizeof(stats))) {
            return -EFAULT;
        }
        return 0;

    case BT_IOC_CLEAR_STATS:
        bt_capture_clear_stats();
        return 0;

    case BT_IOC_GET_FEATURE:
        feature = (struct bt_driver_feature){
            .magic = BT_DRIVER_FEATURE_MAGIC,
            .abi_version = BT_ABI_VERSION,
            .feature_flags = BT_FEATURE_CONTROL_SOCKET | BT_FEATURE_EVENT_STREAM,
            .name = BT_DRIVER_FEATURE_NAME,
        };
        if (copy_to_user(argp, &feature, sizeof(feature))) {
            return -EFAULT;
        }
        return 0;

    default:
        bt_warn("不支持的控制命令: 0x%x\n", cmd);
        return -ENOTTY;
    }
}
