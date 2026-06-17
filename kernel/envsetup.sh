#!/bin/bash
# SPDX-License-Identifier: GPL-2.0-only

export DDK_ROOT=${DDK_ROOT:-/opt/ddk}
export CROSS_COMPILE=${CROSS_COMPILE:-aarch64-linux-gnu-}
export ARCH=${ARCH:-arm64}
export LLVM=${LLVM:-1}
export LLVM_IAS=${LLVM_IAS:-1}

get_clang_for_android() {
    local android_ver="$1"
    case "$android_ver" in
        android12-5.10) echo "clang-r416183b" ;;
        android13-5.10|android13-5.15) echo "clang-r450784e" ;;
        android14-5.15|android14-6.1) echo "clang-r487747c" ;;
        android15-6.6) echo "clang-r510928" ;;
        *) echo "" ;;
    esac
}

detect_ddk_target() {
    for kdir in "$DDK_ROOT/kdir"/android* "$DDK_ROOT/src"/android*; do
        if [ -d "$kdir" ]; then
            basename "$kdir"
            return 0
        fi
    done

    return 1
}

setup_env() {
    local target
    target=$(detect_ddk_target || true)

    if [ -n "$target" ]; then
        local clang_ver
        clang_ver=$(get_clang_for_android "$target")

        if [ -d "$DDK_ROOT/kdir/$target" ]; then
            export KDIR="$DDK_ROOT/kdir/$target"
        elif [ -d "$DDK_ROOT/src/$target" ]; then
            export KDIR="$DDK_ROOT/src/$target"
        fi

        if [ -n "$clang_ver" ] && [ -d "$DDK_ROOT/clang/$clang_ver/bin" ]; then
            export CLANG_PATH="$DDK_ROOT/clang/$clang_ver/bin"
        else
            for clang_dir in "$DDK_ROOT/clang"/clang-*/bin; do
                if [ -d "$clang_dir" ]; then
                    export CLANG_PATH="$clang_dir"
                    break
                fi
            done
        fi

        echo "[envsetup] detected target: $target"
    else
        echo "[envsetup] 未检测到 DDK 目标，回退到 android14-6.1"
        export KDIR="$DDK_ROOT/kdir/android14-6.1"
        export CLANG_PATH="$DDK_ROOT/clang/clang-r487747c/bin"
    fi

    if [ -n "$CLANG_PATH" ] && [ -d "$CLANG_PATH" ]; then
        export PATH="$CLANG_PATH:$PATH"
        export CC="clang"
        export LD="ld.lld"
        export AR="llvm-ar"
        export NM="llvm-nm"
        export OBJCOPY="llvm-objcopy"
        export OBJDUMP="llvm-objdump"
        export STRIP="llvm-strip"
    fi

    echo "[envsetup] KDIR=$KDIR"
    echo "[envsetup] CLANG_PATH=$CLANG_PATH"
    echo "[envsetup] CC=$(command -v clang 2>/dev/null || echo '未找到')"
}

setup_env
