# binder-trace

Binder trace 工具工作区，用于研究和实现 Android Binder 调用观测、内核模块采集、事件解码和 JSONL 输出。

## 目录结构

- `xtask/`: 本地任务和开发命令入口。
- `kernel/`: Android 内核模块、inline hook、符号解析和 DDK 构建脚本。
- `crates/bt-common/`: 内核侧和用户态共享的数据结构，保持小型、稳定、C-layout 友好。
- `crates/bt-agent/`: 用户态核心进程，负责输出启动事件并预留内核模块 reader 接入口。
- `crates/bt-decoder/`: raw Binder 事件流解码层。
- `crates/bt-storage/`: JSONL 输出层，后续可扩展 SQLite。
- `crates/bt-cli/`: 调试 CLI，后续向 stackplz 风格工作流演进。

## 常用命令

```bash
cargo run -p xtask -- check
cargo run -p xtask -- run --output trace.jsonl
cargo run -p bt-cli --bin binder-trace -- --output trace.jsonl
kernel/scripts/build-ddk.sh build android14-6.1
```

Android 设备运行流程:

```bash
export ANDROID_NDK_HOME=/path/to/android-ndk
android/push.sh
android/run-root.sh
```

`android/push.sh` 默认用 debug profile 构建 `aarch64-linux-android` 用户态二进制，并推送到:

- `/data/local/tmp/binder-trace/binder-trace`

当前用户态程序默认只输出程序版本和启动诊断事件；内核模块 reader 接入后会从 `bt-agent`
继续写入 JSONL。

需要看 agent 自身调试日志时，通过 `RUST_LOG` 控制 stderr 输出:

```bash
RUST_LOG=bt_agent=debug android/run-root.sh
RUST_LOG=bt_agent=trace android/run-root.sh
```

需要调整构建或设备路径时可设置:

- `BINDER_TRACE_ANDROID_TARGET`
- `BINDER_TRACE_ANDROID_API`
- `BINDER_TRACE_PROFILE`
- `BINDER_TRACE_REMOTE_DIR`
- `BINDER_TRACE_BIN`

内核模块构建脚本位于 `kernel/scripts/`，默认产物名为 `bt-kmod.ko`。常用命令:

```bash
kernel/scripts/build-ddk.sh build android14-6.1
kernel/scripts/build-ddk.sh clean android14-6.1
```

模块加载辅助脚本为:

```bash
kernel/scripts/insmod_ko.sh bt-kmod.ko
```

当前内核模块安装 Binder hook 后会禁止普通 `rmmod` 热卸载。原因是
`binder_ioctl` 可能长时间阻塞；在 hook 改为 tail-call 形式前，热卸载可能让
阻塞线程返回到已卸载的模块文本段。需要替换模块时先重启测试设备。

## 输出格式

JSONL 输出使用消息信封结构，方便后续写入消息队列。外层携带设备和路由信息，
具体事件内容放在 `data` 下，避免把不同 source 的字段平铺混在一起:

程序启动后会先输出当前程序版本事件:

```json
{
  "device_id": "2957c54c",
  "seq": 0,
  "timestamp_ns": 123456789,
  "object": "program.version",
  "data": {
    "program": "binder-trace",
    "version": "0.1.0"
  }
}
```

后续采集或诊断事件继续使用同一个 `seq` 递增:

```json
{
  "device_id": "2957c54c",
  "seq": 1,
  "timestamp_ns": 123456789,
  "object": "agent.diagnostic",
  "data": {
    "kind": "diagnostic",
    "binder_device": "unknown",
    "process": { "pid": 123, "tid": 0, "uid": 0 },
    "flags": 0,
    "sequence": 0,
    "transaction": null
  }
}
```

内核模块事件 reader 接入后，采集事件会继续复用同一套消息信封，保证 `seq` 单调递增。

## 开发规范

项目文档和代码要求见 [`docs/development-guidelines.md`](docs/development-guidelines.md)。
