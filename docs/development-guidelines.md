# Binder Trace 开发规范

更新时间: 2026-06-16

本文记录项目的文档要求和代码要求。后续实现 Binder/内核模块/内核 ABI 相关逻辑时，必须先满足这里的约束。

## 1. 注释要求

代码注释必须使用中文。允许保留英文专有名词、内核符号名、命令名、类型名和协议字段名，例如 `binder_ioctl`、`BINDER_WRITE_READ`、`struct binder_transaction_data`。

注释只写必要信息，优先说明:

- 为什么这样实现。
- 这个实现依赖哪个 ABI、内核符号或设备能力。
- 这里有什么安全边界、兼容性假设或失败模式。
- 后续维护者修改时必须保持的约束。

不要写机械复述代码的注释，例如“给变量赋值”“调用函数”“返回结果”。如果代码本身已经清楚，宁可不写注释。

## 2. 内核来源要求

凡是代码对接、复刻或推导自内核接口，必须贴出对应的源码位置，避免凭记忆写出虚假的结构体、常量或语义。

适用范围包括:

- Binder UAPI 结构体、常量、命令码和返回码。
- Binder driver 内部函数、tracepoint、inline hook 目标。
- 内核模块读取的内核字段、用户态指针和事件格式。
- 根据内核行为实现的解析、状态关联、fallback 逻辑。

源码位置必须包含:

- 源码仓库或来源，例如 Android common kernel、目标设备 kernel tree、AOSP。
- 分支、tag 或 commit。不能只写“mainline”而不说明当时参考的版本。
- 文件路径。
- 相关符号名，例如结构体、函数、tracepoint、宏。
- 如果引用的是网页，给出可打开的链接。

推荐格式:

```rust
// 内核来源：Android common kernel android-mainline，commit <待填写>
// 路径：include/uapi/linux/android/binder.h
// 符号：struct binder_write_read、struct binder_transaction_data
// 链接：https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/include/uapi/linux/android/binder.h
// 约束：这里不能假设高层 Parcel 参数类型，采集层只传递原始长度和采样 payload。
```

如果来源来自实际设备，也要记录设备上下文:

```text
设备来源：<设备型号>
内核版本：<uname -a 输出>
源码来源：<厂商 kernel tree 或 /sys/kernel/debug/tracing/events/.../format>
采集命令：<用于确认字段的命令>
```

## 3. 文档要求

设计文档必须用中文描述结论、取舍和限制。英文术语可以保留原文，但首次出现时要说明它在本项目里的含义。

涉及内核事实时，必须区分三类内容:

- 已由源码确认的事实。
- 已由设备命令确认的事实。
- 项目设计上的推断或假设。

如果文档里引用内核结构体或 tracepoint 字段，必须紧跟来源链接或在同一节列出来源。不要只粘一段 C 代码而不说明来自哪里。

## 4. 代码要求

### 4.1 依赖要求

实现通用能力时，优先评估成熟库，不要在项目里重复造轮子。例如 bitflags、错误类型、序列化、命令行解析、压缩解压这类已有稳定生态的能力，应先考虑引入库。

新增第三方依赖前，必须先向项目维护者说明:

- 准备引入的库名和用途。
- 为什么标准库或现有依赖不够。
- 该库会进入哪个 crate。
- 对构建、目标设备、许可证和维护成本的影响。

只有维护者明确同意后，才能修改 `Cargo.toml` / `Cargo.lock` 并执行依赖安装或更新命令。未经同意，不得为了绕过审批手写一个已有成熟库覆盖的通用实现。

### 4.2 Crate 边界

`bt-common` 是内核采集侧和用户态共享边界，必须保持简单:

- 优先使用 `#[repr(C)]`、固定宽度整数和小型枚举。
- 避免复杂泛型、堆分配、动态 dispatch 和非必要依赖。
- 共享结构体字段变更时，要同步说明 ABI 影响。

`kernel/` 负责内核态采集，不在内核 hook 路径里做高层 Parcel/AIDL 语义解析。内核侧只做过滤、长度控制、事件打包和必要的状态关联。

用户态 crate 的职责边界:

- `bt-agent` 负责读取内核模块事件、运行用户态编排和融合事件流。
- `bt-decoder` 负责把 raw event 解码成上层事件。
- `bt-storage` 负责持久化，当前优先 JSONL。
- `bt-cli` 负责调试体验，后续可以参考 `stackplz` 的交互方式。

## 5. 提交前检查

提交内核相关代码前，至少检查:

- 新增注释是否为中文。
- 每个内核结构体、常量、tracepoint、hook 点是否有源码位置。
- 是否明确写出参考的 branch/tag/commit。
- 是否避免在内核采集侧解析高层 Parcel/AIDL 语义。
- `bt-common` 是否仍然足够简单，且没有引入不必要依赖。
- 是否跑过 `cargo run -p xtask -- check`。
