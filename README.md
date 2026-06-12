# binder-trace

Binder trace 工具工作区，用于研究和实现 Android Binder 调用观测、eBPF 采集、事件解码和 JSONL 输出。

## 目录结构

- `xtask/`: 本地任务和开发命令入口。
- `crates/bt-common/`: eBPF 和用户态共享的数据结构，保持小型、稳定、C-layout 友好。
- `crates/bt-ebpf/`: 内核态采集层。
- `crates/bt-agent/`: 用户态核心进程，负责加载、运行和融合事件流。
- `crates/bt-decoder/`: raw eBPF 事件流解码层。
- `crates/bt-storage/`: JSONL 输出层，后续可扩展 SQLite。
- `crates/bt-cli/`: 调试 CLI，后续向 stackplz 风格工作流演进。
- `crates/bt-web/`: 后续 Web UI / service 表达层。

## 常用命令

```bash
cargo run -p xtask -- check
cargo run -p xtask -- run --output trace.jsonl
cargo run -p bt-cli --bin binder-trace -- --output trace.jsonl
```

Android 设备运行流程:

```bash
export ANDROID_NDK_HOME=/path/to/android-ndk
android/push.sh
android/run-root.sh
```

`android/push.sh` 默认使用 `aarch64-linux-android` target，并把 `binder-trace` 推送到
`/data/local/tmp/binder-trace/binder-trace`。需要调整时可设置:

- `BINDER_TRACE_ANDROID_TARGET`
- `BINDER_TRACE_ANDROID_API`
- `BINDER_TRACE_PROFILE`
- `BINDER_TRACE_REMOTE_DIR`
- `BINDER_TRACE_BIN`

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

## 开发规范

项目文档和代码要求见 [`docs/development-guidelines.md`](docs/development-guidelines.md)。
