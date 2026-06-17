#!/bin/bash
# SPDX-License-Identifier: GPL-2.0-only

set -e

MODULE_NAME="bt_kmod"
KERNEL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KO_FILE="$KERNEL_DIR/bt-kmod.ko"
REMOTE_DIR="${BINDER_TRACE_REMOTE_DIR:-/data/local/tmp/binder-trace}"
MODULE_PARAMS="$*"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

if [ ! -f "$KO_FILE" ]; then
    log_error "内核模块不存在: $KO_FILE"
    log_info "请先构建: kernel/scripts/build-ddk.sh build"
    exit 1
fi

if ! command -v adb >/dev/null 2>&1; then
    log_error "未找到 adb 命令"
    exit 1
fi

if ! adb devices | grep -q "device$"; then
    log_error "未连接 Android 设备"
    exit 1
fi

if ! adb shell "su -c id" | grep -q "uid=0"; then
    log_error "获取 root 权限失败"
    exit 1
fi

log_info "准备设备目录: $REMOTE_DIR"
adb shell "su -c 'mkdir -p $REMOTE_DIR'"

log_info "推送模块"
adb push "$KO_FILE" "$REMOTE_DIR/" >/dev/null

if adb shell "su -c lsmod | grep -q $MODULE_NAME" 2>/dev/null; then
    log_warn "检测到旧模块仍在运行，尝试卸载；如果有活跃 Binder 调用，rmmod 可能等待较久"
    if adb shell "su -c rmmod $MODULE_NAME" 2>/dev/null; then
        log_info "已卸载旧模块"
    else
        log_error "旧模块卸载失败；如果这是自持有引用的旧版本，请重启设备后再加载新模块"
        adb shell "su -c dmesg | grep -i 'binder-trace' | tail -30" || true
        exit 1
    fi
fi

if [ -n "$MODULE_PARAMS" ]; then
    log_info "加载模块参数: $MODULE_PARAMS"
fi

log_info "加载模块"
if adb shell "su -c 'insmod $REMOTE_DIR/$(basename "$KO_FILE") $MODULE_PARAMS'" 2>/dev/null; then
    log_info "模块加载成功"
else
    log_error "模块加载失败"
    adb shell "su -c dmesg | tail -30"
    exit 1
fi

if adb shell "su -c lsmod | grep -q $MODULE_NAME" 2>/dev/null; then
    adb shell "su -c lsmod | grep $MODULE_NAME"
else
    log_warn "lsmod 中不可见该模块"
fi

log_info "最近的内核日志"
adb shell "su -c dmesg | grep -i 'binder-trace' | tail -20"
