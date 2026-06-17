#!/bin/bash
# SPDX-License-Identifier: GPL-2.0-only

set -e

DEFAULT_TARGET="android12-5.10"
MODULE_NAME="bt-kmod"
KERNEL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

print_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
print_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${CYAN}[成功]${NC} $1"; }

usage() {
    cat << EOF
用法: $0 <command> [options]

使用 DDK 构建、清理和配置 binder-trace 内核模块。

命令:
  build [target]    构建内核模块
  clean [target]    清理构建产物
  compdb [target]   生成 compile_commands.json
  list              列出已安装的 DDK 镜像

选项:
  -t, --target      DDK 目标，默认 android12-5.10
  -s, --strip       去除调试符号
  -h, --help        显示帮助

示例:
  $0 build
  $0 build android14-6.1
  $0 build -t android14-6.1 --strip
  $0 clean android12-5.10
  $0 compdb
  $0 list
EOF
}

check_ddk_installed() {
    if ! command -v ddk >/dev/null 2>&1; then
        print_error "ddk 未安装"
        echo "安装命令:"
        echo "  sudo curl -fsSL https://raw.githubusercontent.com/Ylarod/ddk/main/scripts/ddk -o /usr/local/bin/ddk"
        echo "  sudo chmod +x /usr/local/bin/ddk"
        exit 1
    fi
}

check_docker_permission() {
    if ! command -v docker >/dev/null 2>&1; then
        print_error "Docker 未安装"
        exit 1
    fi

    if ! docker info >/dev/null 2>&1; then
        print_error "无法访问 Docker daemon"
        echo "请使用 sudo 运行，或把当前用户加入 docker 组。"
        exit 1
    fi
}

strip_module() {
    local module_file="$1"

    if [ ! -f "$module_file" ]; then
        print_error "模块文件不存在: $module_file"
        return 1
    fi

    print_info "去除调试符号"
    local before
    before=$(du -h "$module_file" | cut -f1)

    if command -v llvm-strip >/dev/null 2>&1; then
        llvm-strip -d "$module_file"
    elif command -v strip >/dev/null 2>&1; then
        strip -d "$module_file"
    else
        print_warn "未找到 strip/llvm-strip，跳过"
        return 0
    fi

    print_success "去符号后大小: $before -> $(du -h "$module_file" | cut -f1)"
}

cmd_build() {
    local target="$DEFAULT_TARGET"
    local strip=false

    while [ $# -gt 0 ]; do
        case "$1" in
            -t|--target)
                target="$2"
                shift 2
                ;;
            -s|--strip)
                strip=true
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                target="$1"
                shift
                ;;
        esac
    done

    check_ddk_installed
    check_docker_permission

    cd "$KERNEL_DIR"

    print_info "构建配置"
    echo "  目标: $target"
    echo "  模块: $MODULE_NAME.ko"
    echo "  目录: $KERNEL_DIR"
    echo "  去符号: $strip"

    if ! ddk list 2>/dev/null | grep -q "$target"; then
        print_warn "本地没有 DDK 镜像，开始拉取: $target"
        ddk pull "$target"
    fi

    print_info "清理旧构建产物"
    ddk clean --target "$target" 2>/dev/null || true

    print_info "构建内核模块"
    ddk build --target "$target"

    if [ ! -f "$MODULE_NAME.ko" ]; then
        print_error "构建报告成功，但没有生成 $MODULE_NAME.ko"
        exit 1
    fi

    print_success "构建成功: $KERNEL_DIR/$MODULE_NAME.ko"
    print_info "模块大小: $(du -h "$MODULE_NAME.ko" | cut -f1)"

    if [ "$strip" = true ]; then
        strip_module "$MODULE_NAME.ko"
    fi
}

cmd_clean() {
    local target="${1:-$DEFAULT_TARGET}"
    check_ddk_installed
    check_docker_permission
    cd "$KERNEL_DIR"
    ddk clean --target "$target"
}

cmd_compdb() {
    local target="${1:-$DEFAULT_TARGET}"
    local image="ghcr.io/ylarod/ddk:$target"

    check_docker_permission

    if ! docker images --format "{{.Repository}}:{{.Tag}}" | grep -q "$image"; then
        check_ddk_installed
        ddk pull "$target"
    fi

    docker run --rm -v "$KERNEL_DIR:/build" -w /build "$image" make compdb KDIR=\$KERNEL_SRC
}

cmd_list() {
    check_ddk_installed
    check_docker_permission
    ddk list
}

case "${1:-}" in
    build)
        shift
        cmd_build "$@"
        ;;
    clean)
        shift
        cmd_clean "$@"
        ;;
    compdb)
        shift
        cmd_compdb "$@"
        ;;
    list)
        cmd_list
        ;;
    -h|--help|help|"")
        usage
        ;;
    *)
        print_error "未知命令: $1"
        usage
        exit 1
        ;;
esac
