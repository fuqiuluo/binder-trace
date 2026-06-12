#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(CDPATH= cd -- "$script_dir/.." && pwd)"

target="${BINDER_TRACE_ANDROID_TARGET:-aarch64-linux-android}"
api="${BINDER_TRACE_ANDROID_API:-23}"
package="${BINDER_TRACE_PACKAGE:-bt-cli}"
bin="${BINDER_TRACE_BIN:-binder-trace}"
profile="${BINDER_TRACE_PROFILE:-debug}"
remote_dir="${BINDER_TRACE_REMOTE_DIR:-/data/local/tmp/binder-trace}"

case "$target" in
    aarch64-linux-android)
        clang_prefix="aarch64-linux-android"
        ;;
    x86_64-linux-android)
        clang_prefix="x86_64-linux-android"
        ;;
    armv7-linux-androideabi)
        clang_prefix="armv7a-linux-androideabi"
        ;;
    i686-linux-android)
        clang_prefix="i686-linux-android"
        ;;
    *)
        echo "unsupported Android target: $target" >&2
        exit 2
        ;;
esac

case "$(uname -s)" in
    Linux)
        host_tag="linux-x86_64"
        ;;
    Darwin)
        case "$(uname -m)" in
            arm64)
                host_tag="darwin-arm64"
                ;;
            *)
                host_tag="darwin-x86_64"
                ;;
        esac
        ;;
    *)
        echo "unsupported host OS for Android NDK detection: $(uname -s)" >&2
        exit 2
        ;;
esac

target_env="$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')"
linker_var="CARGO_TARGET_${target_env}_LINKER"
linker="${!linker_var:-}"

find_ndk_linker() {
    if [ -n "$linker" ] && [ -x "$linker" ]; then
        printf '%s\n' "$linker"
        return 0
    fi

    ndk_candidates=()
    if [ -n "${ANDROID_NDK_HOME:-}" ]; then
        ndk_candidates+=("$ANDROID_NDK_HOME")
    fi
    if [ -n "${ANDROID_NDK_ROOT:-}" ]; then
        ndk_candidates+=("$ANDROID_NDK_ROOT")
    fi
    if [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME/ndk" ]; then
        while IFS= read -r ndk; do
            ndk_candidates+=("$ndk")
        done < <(find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d | sort -Vr)
    fi
    if [ -n "${ANDROID_SDK_ROOT:-}" ] && [ -d "$ANDROID_SDK_ROOT/ndk" ]; then
        while IFS= read -r ndk; do
            ndk_candidates+=("$ndk")
        done < <(find "$ANDROID_SDK_ROOT/ndk" -mindepth 1 -maxdepth 1 -type d | sort -Vr)
    fi
    if [ -d "$HOME/Android/Sdk/ndk" ]; then
        while IFS= read -r ndk; do
            ndk_candidates+=("$ndk")
        done < <(find "$HOME/Android/Sdk/ndk" -mindepth 1 -maxdepth 1 -type d | sort -Vr)
    fi

    for ndk in "${ndk_candidates[@]}"; do
        candidate="$ndk/toolchains/llvm/prebuilt/$host_tag/bin/${clang_prefix}${api}-clang"
        if [ -x "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

if ! linker="$(find_ndk_linker)"; then
    cat >&2 <<EOF
Android NDK linker not found.

Set one of:
  export $linker_var=/path/to/${clang_prefix}${api}-clang
  export ANDROID_NDK_HOME=/path/to/android-ndk
  export ANDROID_NDK_ROOT=/path/to/android-ndk
  export ANDROID_HOME=/path/to/Android/Sdk
  export ANDROID_SDK_ROOT=/path/to/Android/Sdk
EOF
    exit 2
fi

export "$linker_var=$linker"

cargo_args=(build -p "$package" --bin "$bin" --target "$target")
if [ "$profile" = "release" ]; then
    cargo_args+=(--release)
elif [ "$profile" != "debug" ]; then
    echo "unsupported BINDER_TRACE_PROFILE: $profile" >&2
    exit 2
fi

(cd "$repo_root" && cargo "${cargo_args[@]}")

local_bin="$repo_root/target/$target/$profile/$bin"
if [ ! -x "$local_bin" ]; then
    echo "built binary not found: $local_bin" >&2
    exit 1
fi

adb shell "mkdir -p '$remote_dir'"
adb push "$local_bin" "$remote_dir/$bin" >/dev/null
adb shell "chmod 755 '$remote_dir/$bin'"

echo "$remote_dir/$bin"
