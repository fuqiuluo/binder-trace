# Binder Trace 技术选型文档

更新时间: 2026-06-12

## 1. 背景

Binder 是 Android 的核心 IPC 机制。一次跨进程调用会由用户态 `Parcel` 组包，通过 Binder driver 提交到内核，再由内核复制到目标进程的 Binder buffer 中。Binder driver 能看到 transaction 元数据、目标、长度、Binder object/fd/handle 等，但不知道高层 AIDL 方法名和参数语义。

本项目目标是实现一个 Binder trace 工具，用于观测 Android 设备上的 Binder 调用链、频率、延迟、失败、目标服务，以及在可控场景下采集并解码部分或完整 Parcel 数据。

## 2. 目标与非目标

### 2.1 目标

- 采集 Binder transaction 元数据: caller、target、code、flags、data_size、offsets_size、reply/oneway。
- 支持按 pid、uid、进程名、transaction code、目标进程过滤。
- `transaction code` 是正式 transaction/call 事件的硬要求，丢失时只输出诊断事件。
- 支持统计调用频率、耗时、失败、top caller、top service。
- 在权限和内核能力允许时，采集 Parcel payload。
- 以 Binder ioctl command/return stream 作为主事件流，降低对 Binder tracepoint 的依赖。
- 用户态解析 `code -> AIDL method`、`uid -> package`、`pid -> process`。
- 明确标记 payload 是否完整、是否截断、是否丢事件。

### 2.2 非目标

- 不在 MVP 阶段阻断、篡改或重放 Binder 调用。
- 不在 eBPF 程序里完整解析 Parcel/AIDL 参数。
- 不保证所有用户版设备都可运行完整采集能力。
- 不把 Binder tracepoint 作为主采集前提。
- 不把 tracepoint metadata 当成完整 Binder payload。

## 3. 关键事实

### 3.1 Binder ioctl 是通用事件入口

Binder 用户态协议通过 `ioctl(fd, BINDER_WRITE_READ, struct binder_write_read *)` 与内核交换命令。`binder_write_read` 里有两个字节流:

```c
struct binder_write_read {
    binder_size_t write_size;
    binder_size_t write_consumed;
    binder_uintptr_t write_buffer;
    binder_size_t read_size;
    binder_size_t read_consumed;
    binder_uintptr_t read_buffer;
};
```

`write_buffer` 是用户态写给 Binder driver 的 command stream，例如 `BC_TRANSACTION`、`BC_REPLY`、`BC_TRANSACTION_SG`。`read_buffer` 是 Binder driver 返回给用户态的 return stream，例如 `BR_TRANSACTION`、`BR_REPLY`、`BR_TRANSACTION_COMPLETE`。

因此，hook Binder ioctl 可以构造一个更通用的事件流:

```text
process/thread
  -> ioctl(BINDER_WRITE_READ)
  -> write command stream: BC_*
  -> read return stream: BR_*
  -> userspace event merger
```

这条路径不依赖 Binder tracepoint 是否存在或是否开放，是项目的主采集方案。

### 3.2 Binder 有 tracepoint

Android common kernel 的 Binder driver 定义了 `TRACE_SYSTEM binder`，当前 `android-mainline` 分支中可见事件包括:

- `binder:binder_ioctl`
- `binder:binder_command`
- `binder:binder_return`
- `binder:binder_transaction`
- `binder:binder_transaction_received`
- `binder:binder_transaction_alloc_buf`
- `binder:binder_transaction_buffer_release`
- `binder:binder_transaction_fd_send`
- `binder:binder_transaction_fd_recv`
- `binder:binder_update_page_range`

当前内核分支还可能有 `binder_txn_latency_free`、`binder_netlink_report` 等事件。实际可用事件必须以设备上的 `available_events` 和 `events/binder/*/format` 为准。

参考: [Android common kernel binder_trace.h](https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/drivers/android/binder_trace.h)

### 3.3 Binder tracepoint 不包含实际 Parcel payload

`binder_transaction` tracepoint 通常只包含:

```text
debug_id
target_node
to_proc
to_thread
reply
code
flags
```

`binder_transaction_alloc_buf` 可提供:

```text
debug_id
data_size
offsets_size
extra_buffers_size
```

这些足够做调用链、统计和基础延迟分析，但不能直接得到 `Parcel` 内容。

### 3.4 Binder transaction 明确携带长度

用户态通过 `BC_TRANSACTION` / `BC_REPLY` 向内核提交 `struct binder_transaction_data`，其中包含:

```c
struct binder_transaction_data {
    __u32 code;
    __u32 flags;
    binder_size_t data_size;
    binder_size_t offsets_size;
    union {
        struct {
            binder_uintptr_t buffer;
            binder_uintptr_t offsets;
        } ptr;
        __u8 buf[8];
    } data;
};
```

所以内核明确知道主数据长度 `data_size`、对象偏移表长度 `offsets_size`。对于 scatter-gather transaction，`binder_transaction_data_sg` 还会带 `buffers_size`。嵌入的 `BINDER_TYPE_PTR` 对象也有独立 `length`。

参考: [UAPI binder.h](https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/include/uapi/linux/android/binder.h)

### 3.5 Parcel/AIDL 不是完全自描述格式

Binder kernel ABI 只理解 Binder object，例如 handle、binder、fd、ptr buffer。`data.ptr.buffer` 中的高层数据通常是 Android `Parcel`，是 4 字节对齐的二进制序列。它可以包含 interface token、基础类型、String16、数组、Parcelable 等，但参数类型需要结合 AIDL 接口签名和 transaction `code` 才能可靠解释。

因此，完整语义解码应放在用户态完成。

## 4. 方案对比

| 方案 | 能力 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- | --- |
| tracefs/ftrace only | 读取 `events/binder/*` | 最快验证，无需写 BPF | 无 payload，输出解析弱，权限仍受限 | 作为探测和 fallback |
| eBPF hook Binder ioctl | 解析 `BINDER_WRITE_READ` command/return stream | 通用性最好，不依赖 binder tracepoint，可看到 transaction payload 指针和长度 | 需要解析 BC/BR 字节流，入口/出口状态关联复杂 | MVP 主方案 |
| eBPF tracepoint | 采集 Binder 元数据 | 开销低，字段稳定，可补充内核侧 debug_id/分配/失败信息 | tracepoint 不一定可用，标准 tracepoint 无 payload | 后期辅助增强 |
| eBPF kprobe/fentry `binder_transaction` | 读取 `binder_transaction()` 入参和用户态 buffer | 可直接补充 transaction 语义，可按需过滤 | 依赖符号/BTF/内核布局，完整性需校验 | payload/一致性增强 |
| eBPF hook copy path | 读取内核实际 copy 后的数据 | 更接近最终送达内容 | 兼容性差，依赖 Binder 内部函数和结构 | 作为高级模式 |
| 内核模块 | 可自定义行为和输出 | 控制力强 | GKI/KMI、版本、加载权限、稳定性成本高 | 不推荐 MVP |
| patch kernel 加 tracepoint | 最可靠可观测点 | 数据最准确，可定制字段 | 需要刷机/改内核，难覆盖用户设备 | 仅用于实验或厂商集成 |

## 5. 推荐方案

采用以 Binder ioctl 为主、tracepoint 为辅的分层 eBPF 架构:

```text
Binder ioctl hook
    -> BINDER_WRITE_READ parser
    -> BC_* / BR_* event stream
    -> ringbuf events

optional Binder tracepoints
    -> metadata enrichment
    -> debug_id / allocation / failure hints

optional kprobe/fentry binder_transaction
    -> payload and consistency enrichment
    -> chunked payload events

userspace daemon/CLI
    -> attach/load
    -> event merge
    -> pid/uid/package/service resolver
    -> Parcel/AIDL decoder
    -> table/json/top/record 输出
```

### 5.1 MVP 能力

第一阶段先实现 Binder ioctl 事件流，不以 tracepoint 可用为前提:

- hook Binder ioctl，优先选择 `binder_ioctl`，不可用时评估 syscall `ioctl` tracepoint/kprobe fallback。
- 过滤 `BINDER_WRITE_READ`。
- 在 ioctl 入口记录 `write_buffer`、`write_size`、`read_buffer`、`read_size`。
- 在 ioctl 出口读取 `write_consumed`、`read_consumed`。
- 解析已消费的 `BC_*` command stream 和 `BR_*` return stream。
- 输出 JSONL/table
- 按 pid/tid 维护 ioctl entry/exit 状态。
- 按 pid/uid/code 过滤。

MVP 优先解析:

- `BC_TRANSACTION`
- `BC_REPLY`
- `BC_TRANSACTION_SG`
- `BC_REPLY_SG`
- `BR_TRANSACTION`
- `BR_REPLY`
- `BR_TRANSACTION_COMPLETE`
- `BR_FAILED_REPLY`

### 5.2 Binder fd 定位策略

fd 定位取决于实际 hook 点:

- 如果 hook `binder_ioctl(struct file *filp, ...)`，能进入该函数的调用已经属于 Binder driver，不需要用户态提前告诉 eBPF 哪些 fd 是 Binder fd。
- 如果 fallback 到 generic ioctl，例如 `tracepoint/syscalls/sys_enter_ioctl`、`__arm64_sys_ioctl` 或类似 syscall 层 hook，eBPF 只能看到数字 fd，必须由用户态维护 Binder fd 映射。

generic ioctl fallback 的 fd 映射建议:

```text
key:
  tgid        # 进程 id，fd table 通常按进程共享
  fd          # 该进程内的 fd number

value:
  device_kind # binder / hwbinder / vndbinder / binderfs
  inode/dev   # 可选，用于诊断和去重
```

启动流程建议:

```text
1. load eBPF object，创建 maps。
2. 用户态扫描 /proc/<pid>/fd，识别 /dev/binder、/dev/hwbinder、/dev/vndbinder、binderfs 节点。
3. 写入 binder_fd_map。
4. attach eBPF programs，或打开 capture_enabled 开关。
5. 后台持续维护 fd map。
```

这里的关键点是: fd 是进程局部资源，不能只用 fd number 作为全局身份。同一个 `fd=12` 在不同进程里可以指向完全不同的文件，所以 map key 至少要包含 `tgid`。

fd map 还需要处理生命周期:

- `open/openat`: 新增 Binder fd。
- `close`: 删除 fd。
- `dup/dup2/dup3/fcntl(F_DUPFD*)`: 复制 fd 映射。
- `fork/clone`: 子进程可能继承 fd table，需要复制或重新扫描。
- `exec`: fd 可能保留，也可能因 `CLOEXEC` 被关闭，需要重新校验。
- 进程退出: 清理该 `tgid` 下的 fd。

MVP 可以先采用“启动时扫描 + 周期性重扫”的简单策略。要降低漏报，再增加 open/close/dup/fork/exec 的 hook 做增量维护。

注意: 传递 `IBinder` 对象不会产生新的 Binder fd。跨进程传递 Binder 对象时，接收方得到的是本进程 handle table 里的 handle，后续调用仍然通过接收方原有的 `/dev/binder` fd 发送 `BC_TRANSACTION`。只有传递普通文件描述符时，例如 `BINDER_TYPE_FD` / `BINDER_TYPE_FDA`，接收方才会得到新的 Linux fd。

### 5.3 Tracepoint 辅助模式

当设备支持 Binder tracepoint 时，后期可 attach:

- `tracepoint/binder/binder_transaction`
- `tracepoint/binder/binder_transaction_received`
- `tracepoint/binder/binder_transaction_alloc_buf`

这些事件不作为主事件来源，只用于补充分析:

- 用 `debug_id` 关联内核 transaction 生命周期。
- 补充目标 node、buffer 分配大小、失败路径。
- 校验 ioctl stream 解析结果。
- 在 payload 关闭时提供轻量 metadata。

### 5.4 Payload 模式

ioctl 事件流可从 `BC_TRANSACTION` / `BC_REPLY` 的 `binder_transaction_data` 中拿到:

```text
code
flags
data_size
offsets_size
data.ptr.buffer
data.ptr.offsets
```

采集策略:

- 根据 `write_consumed` 只解析内核实际消费的 command。
- 对 `data_size` 和 `offsets_size` 做上限判断。
- 使用 `bpf_probe_read_user()` 分块读取 `data.ptr.buffer` 和 `data.ptr.offsets`。
- 对 `BC_TRANSACTION_SG` / `BC_REPLY_SG` 记录 `buffers_size`，后续再解析 `BINDER_TYPE_PTR` out-of-line buffer。
- 输出 chunk event，由用户态重组。

如果需要更接近内核内部语义，再补充 hook `binder_transaction()` 入口:

- 读取 `struct binder_transaction_data *tr`。
- 读取 `tr->data_size`、`tr->offsets_size`。
- 和 ioctl stream 结果做交叉校验。
- 按 pid/uid/code 过滤

事件字段建议:

```text
txn_id
pid
tid
uid
code
flags
data_size
offsets_size
buffers_size
chunk_kind       # data / offsets / sg
chunk_index
chunk_offset
chunk_len
total_len
truncated
lost_counter
```

### 5.5 完整性判定

用户态只有在满足以下条件时，才能标记一次 payload 为完整:

- `truncated == false`
- `sum(data chunks) == data_size`
- `sum(offset chunks) == offsets_size`
- chunk offset 连续且无重复
- ringbuf/perfbuf 没有报告 lost event
- decoder 校验 offsets array 未越界

否则标记为:

```text
partial
truncated
lost
decode_failed
```

### 5.6 数据一致性限制

如果在 ioctl 入口或 `binder_transaction()` 入口用 `bpf_probe_read_user()` 读取用户态 buffer，读到的是当时的用户态内存。正常 libbinder 调用路径下足够实用，但恶意进程理论上可以并发修改 buffer，造成 eBPF 采集内容与内核随后 copy 的内容不完全一致。

如果必须证明“采集的是内核实际送达内容”，需要 hook 更深的 copy 路径，或者改内核增加专用 tracepoint。该能力不作为 MVP 承诺。

## 6. Android 版本与设备兼容性

Binder tracepoint 是内核能力，不是 Android framework API。不能只根据 Android 版本判断是否可用。

### 6.1 Binder tracepoint 可用性

当前 Android common kernel 提供 Binder tracepoints，但每台设备是否可用取决于:

- 内核是否包含 Binder tracepoint 定义。
- 是否启用 trace event 相关配置。
- tracefs 是否挂载。
- SELinux/权限是否允许访问 `/sys/kernel/tracing`。
- 厂商是否裁剪或修改 Binder driver。

设备探测命令:

```bash
adb shell su -c 'cat /sys/kernel/tracing/available_events | grep "^binder:"'
adb shell su -c 'ls /sys/kernel/tracing/events/binder'
adb shell su -c 'cat /sys/kernel/tracing/events/binder/binder_transaction/format'
```

### 6.2 eBPF 可用性

Android 官方文档说明 Android 包含 eBPF loader 和 library，可在 boot 阶段加载 `/system/etc/bpf/` 下的 eBPF object，也支持 tracepoint/kprobe 类型程序。实际项目如果面向未刷系统的设备，通常仍需要 root/userdebug 权限来动态加载和 attach。

设备探测项:

```bash
adb shell su -c 'ls /sys/fs/bpf'
adb shell su -c 'zcat /proc/config.gz | grep -E "CONFIG_BPF|CONFIG_BPF_SYSCALL|CONFIG_BPF_EVENTS|CONFIG_DEBUG_INFO_BTF"'
adb shell su -c 'cat /proc/kallsyms | grep " binder_ioctl$"'
adb shell su -c 'cat /sys/kernel/tracing/available_filter_functions | grep "^binder_ioctl$"'
adb shell su -c 'cat /sys/kernel/tracing/available_filter_functions | grep "^binder_transaction$"'
```

参考: [AOSP Extend the kernel with eBPF](https://source.android.com/docs/core/architecture/kernel/bpf)

### 6.3 兼容性分级

| 级别 | 条件 | 支持能力 |
| --- | --- | --- |
| L0 | 无 root，tracefs/BPF 不可访问 | 不支持内核侧 trace |
| L1 | 可读 tracefs binder events | ftrace/tracefs 辅助 metadata |
| L2 | 可加载 eBPF，可 hook `binder_ioctl` 或 syscall `ioctl` | Binder ioctl 事件流 |
| L3 | 可读取 ioctl 用户态 buffer | `BC_*` / `BR_*` 解析和 payload 采样 |
| L4 | 可 attach binder tracepoint 或 `binder_transaction` | tracepoint/kprobe 辅助增强 |
| L5 | 可改内核/系统镜像 | 定制 tracepoint 和系统级集成 |

## 7. 权限与安全

该工具默认应按高敏感数据处理:

- Binder payload 可能包含 token、账号、剪贴板、定位、设备 ID、短信、联系人等敏感数据。
- 默认不采 payload，只采 metadata。
- payload 模式必须显式开启。
- 默认限制 `max_capture_bytes`，例如 256B 或 4KB。
- 输出文件默认权限应限制为当前用户可读写。
- 支持按 uid/package allowlist 采集。
- 支持脱敏策略，例如隐藏长字符串、token-like 字段、fd path。

## 8. 技术栈建议

### 8.1 eBPF 程序

优先选择 C + clang + libbpf CO-RE:

- Android 和 Linux 生态成熟。
- tracepoint/kprobe 示例多。
- 更容易控制 BPF verifier 约束。
- 后续可集成 Android build system 的 `bpf {}` 模块。

### 8.2 用户态

用户态 CLI/daemon 推荐 Rust:

- 事件解析、状态关联、JSON 输出、错误处理更稳。
- 可用 `libbpf-rs` 或 `aya`。
- 如果目标优先是 Android 设备端运行，先验证目标 NDK/toolchain 对依赖的支持。

初期也可以用 C/C++ loader 降低 Android 设备端集成难度，把复杂解析放 host 端。

### 8.3 输出格式

建议支持:

- `table`: 人类实时查看。
- `jsonl`: 后处理和测试。
- `record`: 二进制或压缩 JSONL 记录。
- `top`: 聚合视图。

示例:

```json
{
  "ts_ns": 123456789,
  "from_pid": 1234,
  "from_tid": 1235,
  "to_pid": 987,
  "to_tid": 990,
  "code": 42,
  "flags": 1,
  "oneway": true,
  "data_size": 128,
  "offsets_size": 0,
  "payload_state": "metadata_only"
}
```

## 9. 风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 用户版设备无权限 | 无法加载 BPF 或读 tracefs | 明确要求 root/userdebug；提供 host 探测命令 |
| `binder_ioctl` 符号不可见 | 无法走 Binder 专用 kprobe | fallback 到 syscall `ioctl`，按 fd/device 过滤 |
| Binder fd map 过期 | generic ioctl fallback 漏报或误报 | 启动扫描 + 周期性重扫；后续 hook open/close/dup/fork/exec |
| ioctl stream 解析错误 | 事件错位或漏报 | 严格按 UAPI command size 解析，保留 raw command 诊断 |
| 厂商内核裁剪 tracepoint | 无法获得辅助 metadata | tracepoint 只作为增强，不影响 ioctl 主事件流 |
| 无 BTF/符号 | kprobe/fentry 兼容性下降 | 优先 ioctl stream；增强模式再做 per-kernel fallback |
| ringbuf 丢事件 | payload 不完整 | lost counter + 完整性状态 |
| payload 过大 | 高开销、丢事件 | max_capture、采样、过滤 |
| Parcel 语义 gap | 只能看到 code 和 bytes | 用户态 AIDL 签名表和 service resolver |
| 隐私泄露 | 高安全风险 | 默认关闭 payload，allowlist，脱敏，权限控制 |

## 10. 验证计划

### 10.1 设备能力探测

```bash
adb shell su -c 'uname -a'
adb shell su -c 'cat /proc/kallsyms | grep " binder_ioctl$"'
adb shell su -c 'cat /sys/kernel/tracing/available_filter_functions | grep "^binder_ioctl$"'
adb shell su -c 'ls -l /dev/binder /dev/vndbinder /dev/hwbinder 2>/dev/null'
adb shell su -c 'ls /sys/fs/bpf'
adb shell su -c 'cat /sys/kernel/tracing/available_events | grep "^binder:"'
adb shell su -c 'cat /sys/kernel/tracing/events/binder/binder_transaction/format'
adb shell su -c 'cat /proc/kallsyms | grep " binder_transaction$"'
```

### 10.2 Ioctl Stream MVP 验证

- 调用 `service list`、`cmd activity`、`settings get` 触发 Binder 调用。
- 检查是否能记录 `BINDER_WRITE_READ` ioctl entry/exit。
- 如果走 generic ioctl fallback，检查用户态扫描出的 `(tgid, fd)` 是否覆盖目标进程的 `/dev/binder`、`/dev/hwbinder`、`/dev/vndbinder`。
- 检查是否能解析 `BC_TRANSACTION`、`BC_REPLY`、`BR_TRANSACTION`、`BR_REPLY`。
- 检查 `write_consumed`、`read_consumed` 是否与已解析 stream 长度一致。
- 在 tracepoint 可用设备上，对比 tracefs `trace_pipe` 输出和 ioctl stream 输出。
- 压测高频 Binder 调用，观察 lost event。

### 10.3 Payload 验证

- 构造自定义 AIDL demo service。
- 发送固定参数，例如 int、String、byte array。
- 通过 `data_size` 和 chunk 重组校验完整性。
- 用户态 decoder 校验 interface token 和参数。
- 测试大 payload、fd、binder object、SG transaction。

## 11. 最终决策

项目采用:

```text
eBPF Binder ioctl event-stream collector
+ multi-source capability fusion
+ Binder tracepoint / binder_transaction / syscall fd-map enrichment
+ userspace resolver/decoder
```

不采用内核模块作为默认方案，不修改 Binder driver 作为 MVP 前提。

理由:

- 能覆盖 Binder 观测的大部分需求。
- 不需要刷机或改内核即可在 root/userdebug 设备上验证。
- ioctl 是 Binder 用户态协议的通用入口，比 tracepoint 更适合作为主事件流。
- tracepoint 不一定在目标设备上可用，因此不能作为唯一前提；但可用时应作为 code、debug_id、target metadata 的能力 provider。
- kprobe/fentry 和 copy path 可以补足 payload 一致性分析能力。
- 用户态解码更适合处理 AIDL/Parcel 的语义复杂度。
- 内核模块和 kernel patch 的维护成本明显高于当前目标收益。

详细能力融合与回退路径见 [Binder Trace 能力融合与回退设计](fallback-design.md)。核心策略不是只选择一个 source，而是尽量同时启用可用 provider 来补齐四个能力: 区分 Binder 请求、获取 transaction `code`、采集请求体、关联返回。其中 `transaction code` 是硬门禁；如果所有 provider 都无法提供 code，则不输出正式 Binder transaction/call 事件。

## 12. 设备实测记录

### 12.1 Xiaomi mondrian / Android 14 / kernel 5.10.177

设备信息:

```text
model: 23013RK75C
device: mondrian
Android: 14
kernel: 5.10.177-android12-9-g6e14cdf13edc
SELinux: Enforcing
root context: u:r:su:s0
```

实测结论:

- `/dev/binder`、`/dev/hwbinder`、`/dev/vndbinder` 均来自 binderfs，主设备号为 234。
- 设备存在 Binder tracepoint，包括 `binder:binder_ioctl`、`binder:binder_transaction`、`binder:binder_command`、`binder:binder_return`。
- `syscalls:sys_enter_ioctl` / `syscalls:sys_exit_ioctl` 不可用。
- `/proc/kallsyms` 中 `binder_ioctl` 带 CFI hash，例如 `binder_ioctl$07b94060fb3ca59cd1d7a45c168e567d`。
- kprobe `binder_ioctl$...` 可用，能稳定读取 `cmd=0xc0306201`、`arg`、`binder_write_read` 字段。
- 从 `write_buffer` 可读取第一个 `BC_*` command，并能在 `BC_TRANSACTION` / `BC_REPLY` 场景读取 `txn_code`、`txn_flags`、`data_size`、`offsets_size`、`data.ptr.buffer`。
- kprobe `binder_ioctl_write_read` 在该设备上用 `$arg3`/`%x2` 读到的是内核地址，不适合作为当前设备的主 hook 点。
- kprobe `__arm64_sys_ioctl` 的 direct args 和 pt_regs 偏移读取结果不可靠，generic ioctl fallback 需要后续单独适配。

已落地复现脚本:

```bash
tools/probe_binder_ioctl_tracefs.sh 1
```

脚本会动态解析 `binder_ioctl$...` 符号，创建 tracefs kprobe，抓取 `BINDER_WRITE_READ`，输出最近 80 条 `binder_ioctl_cmd` 事件，并在退出时清理 kprobe event。

已落地 eBPF 原型:

```bash
tools/run_ebpf_binder_ioctl_printk.sh 1
```

该原型使用 NDK 编译设备端 loader，运行时:

- 通过 kprobe PMU attach 到 `binder_ioctl$...`。
- 加载 `BPF_PROG_TYPE_KPROBE` 程序。
- eBPF 从 ARM64 `pt_regs` 读取 `x1=cmd`、`x2=arg`。
- eBPF 使用 `bpf_probe_read_user` 读取用户态 `struct binder_write_read`。
- eBPF 继续读取 `write_buffer` 首个 `BC_*` command，并打印 `bc0`、`txn_code`、`data_size` 到 `bpf_trace_printk`。

实测输出示例:

```text
bpf_trace_printk: bt 40406300 3 244
bpf_trace_printk: bt 40086303 15 92
```

含义:

- 第一列是 `bc0`，例如 `0x40406300`。
- 第二列是按 transaction 布局读取的 `txn_code`。
- 第三列是 `data_size`。

当前 eBPF 原型为验证用途，使用 `bpf_trace_printk` 输出，并对每个 CPU 打开一个 kprobe perf fd，所以同一事件可能重复输出。正式实现应改为 ringbuf/perfbuf 输出，并按实际 attach 语义避免重复事件。

## 13. 参考资料

- [Android Binder overview](https://source.android.com/docs/core/architecture/ipc/binder-overview)
- [Android common kernel binder_trace.h](https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/drivers/android/binder_trace.h)
- [Android common kernel binder UAPI](https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/include/uapi/linux/android/binder.h)
- [Android common kernel binder.c](https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/drivers/android/binder.c)
- [AOSP: Extend the kernel with eBPF](https://source.android.com/docs/core/architecture/kernel/bpf)
- [Linux kernel: Event Tracing](https://www.kernel.org/doc/html/latest/trace/events.html)
