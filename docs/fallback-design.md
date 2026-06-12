# Binder Trace 能力融合与回退设计

更新时间: 2026-06-12

## 1. 设计目标

Binder trace 的核心目标是尽量保持同一个抽象事件流:

```text
ioctl(BINDER_WRITE_READ)
  -> binder_write_read
  -> write_buffer: BC_* command stream
  -> read_buffer: BR_* return stream
  -> userspace resolver / decoder
```

但 Android 设备内核差异很大，不能假设 `BPF_PROG_TYPE_KPROBE`、Binder tracepoint、syscall tracepoint、BTF 同时存在。因此这里的“回退”不是严格的一级一级替换关系，而是多信号源相辅相成: 能挂上的 source 尽量同时启用，再按能力补齐同一个 Binder event。

项目需要尽可能满足四个核心能力:

| 编号 | 能力 | 要求 |
| --- | --- | --- |
| C1 | 区分是不是 Binder 请求 | 必须尽量可靠，允许通过 driver hook、Binder tracepoint 或 fd map 判断 |
| C2 | 获取 transaction `code` | 硬要求，丢失时不能输出正式 transaction 事件 |
| C3 | 获取请求体 | 尽量满足，可受采样上限和权限限制 |
| C4 | 捕获对应返回 | 尽量满足，同步调用需要关联 request/reply，oneway 没有 reply |

其中 C2 是 hard requirement。没有 `code` 的数据只能作为诊断事件或原始观测事件，不能进入上层的 Binder transaction/call 模型。

## 2. 输出能力分级

工具对外应该明确标记当前采集等级和核心能力位，而不是假装所有设备能力一致。

| 等级 | 名称 | 能力 |
| --- | --- | --- |
| S0 | unsupported | 无法做内核侧 Binder trace |
| S1 | metadata-only | 只能采 Binder transaction metadata，无 ioctl stream/payload |
| S2 | ioctl-intent | 能看到 `BINDER_WRITE_READ` 的 `cmd/arg/write_size/read_size`，但不能可靠确认 consumed |
| S3 | ioctl-write-stream | 能解析 `BC_*` write command stream |
| S4 | ioctl-read-write-stream | 能解析 `BC_*` 和 `BR_*` stream |
| S5 | payload-sample | 能采样 `data.ptr.buffer` / `offsets` |
| S6 | enriched | 能结合 tracepoint/kprobe/fentry 补充 debug_id、目标进程、buffer 分配、失败路径 |

正式事件里建议带:

```text
source        # kprobe:binder_ioctl / tracepoint:binder_ioctl / tracepoint:binder_transaction / syscall:ioctl / ...
capability    # S1/S2/S3/...
features      # binder_identity / transaction_code / request_payload / reply_correlation
confidence    # exact / best_effort / metadata_only
code_state    # exact / inherited / missing
truncated
lost_counter
```

## 3. 信号源能力矩阵

P0/P1/P2 这些编号只表示 source 类型，不表示只能选择其中一个。实际运行时建议按设备能力启用多个 source:

```text
ioctl source        -> 负责 Binder 判定、code、payload、BR/BC stream
metadata tracepoint -> 负责 debug_id、target proc/thread、内核路径状态
syscall source      -> 负责 fd number、fd 生命周期、跨设备通用兜底
fentry/fexit        -> 负责低开销参数和 return 补充
```

事件融合后再判断 C1-C4 是否满足。`transaction code` 没有任何 source 能提供时，当前设备只能进入 diagnostic mode。

| Source | C1 Binder 判定 | C2 transaction code | C3 请求体 | C4 对应返回 | 主要用途 |
| --- | --- | --- | --- | --- | --- |
| P0 kprobe:binder_ioctl | 是，driver hook 天然确定 | 是，从 `BC_TRANSACTION` 解析 | 是，从 `data.ptr.buffer` 采样 | 需要 kretprobe/fexit 解析 `BR_REPLY`/`BC_REPLY` 并结合线程状态 | 主 ioctl stream |
| P1 tracepoint:binder_ioctl | 是，Binder tracepoint 天然确定 | 是，从 `arg -> write_buffer` 解析 | 是，从 `data.ptr.buffer` 采样 | 需要 ioctl done/read done 或其他 exit-side source | kprobe 不可用时保留 code/payload |
| P2 binder metadata tracepoints | 是 | 是，`binder_transaction` 字段 | 否 | 部分，依赖 debug_id/received/reply 字段 | code 的重要兜底和 enrichment |
| P3 syscall:ioctl tracepoint | 需要 fd map | 是，从 `arg -> write_buffer` 解析 | 是 | 需要 sys_exit 解析 read_buffer/ret | fd 和通用 ioctl 兜底 |
| P4 raw_tracepoint:sys_enter | 需要 fd map | 是，从寄存器 `arg -> write_buffer` 解析 | 是 | 需要 raw sys_exit/状态表 | syscall tracepoint 缺失时兜底 |
| P5 fentry/fexit:binder_ioctl | 是，driver function | 是，从 `arg -> write_buffer` 解析 | 是 | 是/部分，fexit 可补 ret | BTF 存在时增强 |
| P6 tracefs/ftrace | 取决于 event | 取决于 binder_transaction format | 通常否 | 部分 | 诊断和非 eBPF 兜底 |

`code` 来源优先级:

1. ioctl stream 中 `BC_TRANSACTION` 的 `struct binder_transaction_data.code`。
2. Binder `binder_transaction` tracepoint 的 `code` 字段。
3. syscall/raw tracepoint 中 ioctl `arg` 解析出的 `code`。

不要从 payload 内容反推 `code`。如果以上来源都没有成功，正式事件必须标记为 unsupported/diagnostic，而不是输出缺 code 的 transaction。

### 3.1 P0: eBPF kprobe `binder_ioctl`

高价值 source:

```text
BPF_PROG_TYPE_KPROBE
perf kprobe PMU
symbol: binder_ioctl 或 binder_ioctl$<cfi_hash>
```

可获得:

- `struct file *filp`
- `cmd`
- `arg`
- `binder_write_read`
- `write_buffer` / `read_buffer`
- `BC_*` / `BR_*` stream
- `data_size` / `offsets_size` / payload pointer

优点:

- 已经确定是 Binder driver，不需要 fd map。
- 能从 `filp` 区分 binder/hwbinder/vndbinder/binderfs 节点。
- 能保留项目主事件流。
- 可与 Binder metadata tracepoints 同时启用，用 metadata 补 `debug_id/to_proc/to_thread`。

缺点:

- 依赖 `BPF_PROG_TYPE_KPROBE`、kprobe PMU、目标符号。
- 符号可能被裁剪、inline、CFI/LTO 改名或 blacklist。

当前实测设备可用，命令:

```bash
tools/run_ebpf_binder_ioctl_printk.sh 1
```

### 3.2 P1: eBPF tracepoint `binder:binder_ioctl`

当 `BPF_PROG_TYPE_KPROBE` 不可用，但 Binder tracepoint 可用时，优先启用该 source；即使 kprobe 可用，它也可以作为低成本校验或补充:

```text
BPF_PROG_TYPE_TRACEPOINT
tracepoint/binder/binder_ioctl
```

该 tracepoint 常见字段:

```text
cmd
arg
```

可获得:

- `cmd`
- `arg`
- 通过 `arg` 读取用户态 `struct binder_write_read`
- 通过 `write_buffer` 读取 `BC_*` write command stream
- payload 指针和大小

优点:

- 不依赖 kprobe。
- `binder:binder_ioctl` 如果存在，字段比 syscall wrapper 更贴近 Binder。
- 仍可保留大部分 ioctl write stream 设计。
- 仍可提供 transaction `code`，这是它比纯 metadata fallback 更关键的价值。

限制:

- tracepoint 不一定存在，厂商可裁剪。
- `binder:binder_ioctl` 不带 `struct file *filp`，不能直接区分 binder/hwbinder/vndbinder。
- 不直接提供 fd number。
- 不直接提供 ioctl return 时的 `write_consumed/read_consumed`。如果没有额外 exit hook，只能按 `write_size/read_size` 做 intent-level 解析，无法证明 driver 实际消费了多少。
- 仍然是读取用户态 buffer，和 kprobe 入口一样存在 TOCTOU 风险。

适配策略:

```text
if binder:binder_ioctl exists and BPF_PROG_TYPE_TRACEPOINT load succeeds:
  attach binder_ioctl tracepoint
  filter cmd == BINDER_WRITE_READ
  bpf_probe_read_user(arg -> binder_write_read)
  parse write_buffer up to write_size/max_capture
  mark capability = S3 or S5
  mark confidence = best_effort
```

如果还存在:

```text
tracepoint/binder/binder_ioctl_done
tracepoint/binder/binder_write_done
tracepoint/binder/binder_read_done
```

可以补充 ret/error 信息，但通常仍不足以完整还原 `write_consumed/read_consumed`，除非目标内核 tracepoint format 明确暴露这些字段。

### 3.3 P2: eBPF Binder metadata tracepoints

Binder metadata tracepoints 不只是 fallback，也应该作为 P0/P1/P3/P4 的辅助 source。它们不能提供 payload，但经常能提供 `code`、`debug_id`、目标进程、线程和内核路径状态:

```text
tracepoint/binder/binder_transaction
tracepoint/binder/binder_transaction_received
tracepoint/binder/binder_transaction_alloc_buf
tracepoint/binder/binder_transaction_fd_send
tracepoint/binder/binder_transaction_fd_recv
tracepoint/binder/binder_command
tracepoint/binder/binder_return
```

可获得:

- transaction `debug_id`
- `target_node`
- `to_proc`
- `to_thread`
- `reply`
- `code`
- `flags`
- `data_size`
- `offsets_size`
- fd send/recv metadata
- command/return code

优点:

- 不需要 kprobe。
- 对调用图、频率、基础 transaction 统计很有用。
- 可以作为 P0/P1/P3/P4 的 enrichment。
- 在 ioctl source 不可用时，仍可能满足 C1/C2，保持 metadata-only transaction 事件。

限制:

- 标准 `binder_transaction` tracepoint 不包含 Parcel payload。
- 无法完整重建 ioctl command stream。
- 不能拿到 `data.ptr.buffer` 实际 bytes。

能力标记:

```text
capability = S1
confidence = metadata_only
features.transaction_code = true if binder_transaction format has code
```

### 3.4 P3: syscall tracepoint `sys_enter_ioctl`

如果 syscall tracepoint 可用，它不必等 Binder tracepoint 失败才启用。它的独特价值是 fd number 和 fd 生命周期，可辅助区分 binder/hwbinder/vndbinder:

```text
BPF_PROG_TYPE_TRACEPOINT
tracepoint/syscalls/sys_enter_ioctl
tracepoint/syscalls/sys_exit_ioctl
```

可获得:

- `fd`
- `cmd`
- `arg`
- ioctl return value
- 通过 fd map 判断该 fd 是否 Binder device
- 通过 `arg` 读取 `binder_write_read`

优点:

- syscall tracepoint 是很多 eBPF syscall tracing 工具的标准路径。
- 能看到用户态 fd number。
- 能做 fd map，区分 binder/hwbinder/vndbinder。
- 可作为 P1 缺少 `filp/fd` 时的 device_kind 补充。

限制:

- 很多 Android 设备不开 `CONFIG_FTRACE_SYSCALLS`，本项目实测设备就没有 `syscalls:sys_enter_ioctl`。
- 需要维护 `(tgid, fd) -> device_kind`。
- fd 生命周期复杂: open/close/dup/fork/exec 都要处理。
- syscall 层会看到所有 ioctl，需要强过滤。

能力标记:

```text
capability = S3/S4/S5
confidence = best_effort
```

### 3.5 P4: raw tracepoint `sys_enter`

如果 syscall tracepoint 缺失，但 raw tracepoint 可用:

```text
BPF_PROG_TYPE_RAW_TRACEPOINT
raw_tracepoint/sys_enter
raw_tracepoint/sys_exit
```

可获得:

- syscall id
- 寄存器参数
- ioctl fd/cmd/arg

优点:

- 比 syscall tracepoint 更底层，部分内核可能可用。
- 开销较低。

限制:

- 参数解析更依赖架构 ABI。
- Android 厂商内核可能有 wrapper/compat/CFI 差异。
- 仍需要 fd map。

能力标记:

```text
capability = S3/S4/S5
confidence = best_effort
```

### 3.6 P5: fentry/fexit `binder_ioctl`

如果 BTF/fentry 可用，可以尝试；它既可以替代 kprobe，也可以在特定设备上作为更低开销的 driver hook:

```text
BPF_PROG_TYPE_TRACING
attach_type = BPF_TRACE_FENTRY / BPF_TRACE_FEXIT
target = binder_ioctl
```

优点:

- 性能好。
- 参数类型更清晰。
- fexit 能拿到返回值。

限制:

- 依赖 BTF 和目标函数 BTF 信息。
- Android 设备上兼容性通常不如 kprobe/tracepoint。
- CFI/LTO/厂商裁剪仍可能影响目标函数可见性。

能力标记:

```text
capability = S4/S5/S6
confidence = exact 或 best_effort
```

### 3.7 P6: tracefs/ftrace 非 eBPF fallback

如果 eBPF attach 被 SELinux 或内核策略拒绝，但 tracefs 可读:

```text
/sys/kernel/tracing/events/binder/*
/sys/kernel/tracing/trace_pipe
```

可获得:

- Binder metadata trace。
- 可能可用动态 kprobe event。

用途:

- 诊断和设备能力探测。
- 非正式 MVP fallback。

限制:

- 不是 eBPF 技术路径。
- 输出解析弱。
- 性能和权限模型不适合作为最终长期方案。

## 4. 启动探测流程

建议用户态 loader 启动时按下面顺序探测。探测顺序不是运行时 source 的唯一选择顺序，每个 source 都应输出 `available/attached/provided_features`。

```text
1. 基础权限
   - root/su
   - /sys/fs/bpf
   - bpf(BPF_PROG_LOAD) smoke test

2. P0: binder_ioctl kprobe
   - /sys/bus/event_source/devices/kprobe/type
   - /proc/kallsyms 查 binder_ioctl 或 binder_ioctl$*
   - BPF_PROG_TYPE_KPROBE load smoke test
   - perf_event_open kprobe PMU
   - PERF_EVENT_IOC_SET_BPF

3. P1: binder_ioctl tracepoint
   - available_events 是否有 binder:binder_ioctl
   - 读取 events/binder/binder_ioctl/format，确认 cmd/arg 字段
   - BPF_PROG_TYPE_TRACEPOINT load smoke test
   - attach tracepoint

4. P2: binder metadata tracepoints
   - binder_transaction / binder_transaction_alloc_buf / binder_command / binder_return
   - 读取 format，构建字段 decoder

5. P3: syscall tracepoint
   - syscalls:sys_enter_ioctl / syscalls:sys_exit_ioctl
   - fd map 初始化

6. P4: raw tracepoint
   - raw_tracepoint/sys_enter
   - 架构 ABI decoder

7. P5: fentry/fexit
   - /sys/kernel/btf/vmlinux
   - BTF 查 binder_ioctl
   - BPF_PROG_TYPE_TRACING load/attach
```

探测完成后按能力启用:

```text
1. 建立 C1/C2/C3/C4 -> providers 映射
2. 如果没有任何 C2 provider:
   - mode = diagnostic
   - 不输出正式 Binder transaction/call 事件
3. 至少启用一个 C1+C2 provider:
   - P0/P5 优先，因为能同时提供 driver identity、code、payload
   - P1 可在无 kprobe 时提供 Binder identity、code、payload
   - P2 可在无 ioctl stream 时提供 Binder identity、code metadata
   - P3/P4 需要 fd map 才能提供 Binder identity
4. 如果用户开启 payload:
   - 需要 P0/P1/P3/P4/P5 之一提供 C3
   - 超过采样上限时标记 truncated，不影响 code
5. 如果用户开启 reply correlation:
   - 解析 `BR_REPLY` / `BC_REPLY`
   - 结合 tid、同步调用栈、debug_id、时间窗口做关联
   - 关联失败的 reply 只能作为 unpaired reply observation
```

探测结果建议输出为 JSON:

```json
{
  "mode": "full",
  "enabled_sources": [
    "kprobe:binder_ioctl",
    "tracepoint:binder_transaction"
  ],
  "capability": "S6",
  "capabilities": {
    "binder_identity": true,
    "transaction_code": true,
    "request_payload": true,
    "reply_correlation": "best_effort"
  },
  "providers": {
    "binder_identity": ["kprobe:binder_ioctl", "tracepoint:binder_transaction"],
    "transaction_code": ["kprobe:binder_ioctl", "tracepoint:binder_transaction"],
    "request_payload": ["kprobe:binder_ioctl"],
    "reply_correlation": ["kprobe:binder_ioctl"]
  },
  "hard_requirements": {
    "transaction_code": true
  },
  "probed_sources": {
    "kprobe": "attached",
    "binder_ioctl_tracepoint": "available",
    "binder_transaction_tracepoint": "attached",
    "syscall_ioctl_tracepoint": "unavailable",
    "raw_tracepoint": "unknown",
    "btf": "unavailable"
  },
  "notes": [
    "binder_ioctl symbol has CFI suffix",
    "syscall ioctl tracepoint unavailable"
  ]
}
```

## 5. 统一事件模型

不同 source 先输出原始 observation，再由用户态 fusion 层合并成内部事件。正式 transaction event 必须满足 `code_state != missing`。

```text
BinderEvent {
  event_type              # raw_observation / request / reply / transaction / diagnostic
  source
  capability
  features
  confidence
  ts_ns
  pid
  tid
  uid
  comm
  device_kind
  fd
  cmd
  bc_or_br
  target_handle
  target_ptr
  code
  code_state              # exact / inherited / missing
  code_source
  flags
  data_size
  offsets_size
  payload_state
  payload_sample
  kernel_debug_id
  to_proc
  to_thread
  correlation_key
  reply_of
  reply_state             # none / pending / matched / unpaired / oneway
  ret
  lost_counter
}
```

字段来源规则:

- `device_kind`: P0 从 `filp`/inode/context 读取；P3/P4 从 fd map；P1/P2 可能为空。
- `fd`: 只有 P3/P4 syscall 层天然有；P0/P1 没有。
- `code`: P0/P1/P3/P4/P5 从 `BC_TRANSACTION` 的 `binder_transaction_data.code` 读取；P2 从 `binder_transaction` tracepoint 读取。
- `code_state`: request 事件必须是 `exact`；reply 事件如果从已匹配 request 继承 code，则是 `inherited`；无法匹配 request 的 reply 只能作为 diagnostic/unpaired observation。
- `payload_sample`: P0/P1/P3/P4 可从用户态 buffer 采样；P2 不支持。
- `kernel_debug_id/to_proc/to_thread`: P2/P6 metadata tracepoints 提供；P0/P1 需要额外 enrichment。
- `reply_state`: 同步请求进入 `pending`；看到对应 `BR_REPLY` 或服务端 `BC_REPLY` 后变成 `matched`；`TF_ONE_WAY` 请求直接标为 `oneway`。
- `ret`: syscall exit、kretprobe/fexit、binder_ioctl_done 等 source 才能提供。

## 6. 返回关联模型

C4 的目标是捕获“对应的返回”，这里需要区分三个事件:

```text
client thread:
  BC_TRANSACTION(code=N)  -> request
  BR_REPLY                -> reply delivered to caller

server thread:
  BR_TRANSACTION(code=N)  -> request delivered to callee
  BC_REPLY                -> callee sends reply
```

入口侧 `binder_ioctl` 只能可靠看到用户态写入的 `write_buffer`。如果要解析 driver 写回给用户态的 `read_buffer`，需要 exit-side source:

```text
kretprobe/fexit:binder_ioctl
sys_exit_ioctl
binder_ioctl_done / binder_read_done tracepoint, if format exposes enough data
```

关联策略:

- 发起端同步调用: 捕获 `BC_TRANSACTION` 后，在 `(tgid, tid)` 上压入 pending request；同线程后续 `BR_REPLY` 继承该 request 的 `code`。
- oneway 调用: `TF_ONE_WAY` 不等待业务 reply，直接标记 `reply_state = oneway`。
- 服务端视角: `BR_TRANSACTION` 携带 request 的 `code`；服务端发送 `BC_REPLY` 时通常不能依赖 reply 自身携带原始业务 `code`，需要通过服务端线程 pending transaction 或 Binder metadata 关联。
- metadata 辅助: `binder_transaction` / `binder_transaction_received` 的 `debug_id`、`to_proc`、`to_thread` 可用于修正单纯按 `(tgid, tid)` 和时间窗口关联的不确定性。
- 关联失败: reply 只能输出 `reply_state = unpaired` 的 observation，不能伪造或猜测 `code`。

因此 C4 可以 best effort，但不能牺牲 C2。reply 如果无法关联到 request，就不能进入带 `code` 的正式 call 结果。

## 7. 能力缺口和补齐策略

| 能力 | 首选 provider | 可补齐 provider | 不能满足时的行为 |
| --- | --- | --- | --- |
| C1 Binder 判定 | P0/P5 driver hook | P1/P2 Binder tracepoint，P3/P4 fd map | 不输出正式 Binder 事件 |
| C2 transaction code | P0/P1/P3/P4/P5 ioctl stream | P2 `binder_transaction.code` | 硬失败，进入 diagnostic mode |
| C3 请求体 | P0/P1/P3/P4/P5 读取 `data.ptr.buffer` | 无，P2 只能给 size metadata | 降为 metadata-only，但保留 code |
| C4 对应返回 | P0/P1/P3/P4/P5 解析 `BR_REPLY`/`BC_REPLY` | P2 debug_id/received/reply metadata | 输出 request，reply 标记 pending/unpaired |

特殊规则:

- 如果只有 P2 可用且 `binder_transaction` format 有 `code`，可以输出 metadata-only transaction，不能输出 payload。
- 如果只有 P1 可用，仍然可以满足 C1/C2/C3，但 device_kind 可能为空，reply 关联是 best effort。
- 如果只有 P3/P4 可用，必须先建立 fd map，否则 C1 不成立；即使能解析 `code`，也不能证明该 ioctl 是 Binder。
- 如果 reply 无法关联回 request，不能给它编造 `code`，只能输出 `reply_state = unpaired` 的 observation。

## 8. 当前项目决策

默认启用集合:

```text
P0 kprobe:binder_ioctl
P2 binder metadata tracepoints
```

如果 `BPF_PROG_TYPE_KPROBE` 不可用:

```text
P1 tracepoint:binder_ioctl
P2 binder metadata tracepoints
```

原因:

- 它仍然保留 `cmd/arg`，可以继续读取 `binder_write_read`。
- 它可以继续提供 transaction `code`，满足 C2 硬要求。
- P2 可补 `debug_id/to_proc/to_thread`，也可在 P1 读取失败时保住 metadata code。

如果 `binder:binder_ioctl` 不可用，但 metadata tracepoint 可用:

```text
P2 binder metadata tracepoints
```

这时能力降级为 metadata-only，不再承诺 payload/Parcel 采样；但只要 `binder_transaction` format 有 `code`，仍可输出正式 metadata transaction。

如果设备没有 Binder tracepoint，但有 syscall tracepoint/raw tracepoint:

```text
P3/P4 syscall ioctl + fd map
```

这条路径工程复杂度较高，但不是“最后才有意义”的降级项；它可以和 P1/P2 并存，用来补 fd number、device_kind 和 open/dup/close 生命周期。

最终门禁:

```text
if !capabilities.transaction_code:
  mode = diagnostic
  do not emit Binder transaction/call event
```

`transaction code` 丢失不可接受。其他能力可以降级、标记 best_effort 或 metadata_only，但不能用缺 code 的事件进入正式分析链路。
