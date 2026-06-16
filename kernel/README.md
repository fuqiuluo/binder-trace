# binder-trace 内核模块

这个目录存放 `binder-trace` 的 Android out-of-tree 内核模块后端。

## 当前 hook

模块加载时会通过 `kallsyms_lookup_name` 解析 Binder 相关符号，然后安装 ARM64 inline hook：

- `binder_ioctl`：必需 hook，用来拿到 `filp`、`cmd`、`arg`。
- `binder_alloc_copy_user_to_buffer`：必需 hook，用来拿到 Binder 写入用户态缓冲区前的入口参数。
- `binder_transaction`：可选 hook。这个函数是 `static`，部分内核不会出现在 kallsyms 里，解析失败时模块会跳过它。

跨版本策略：hook 层只读取函数入口参数，不解引用 `struct binder_proc`、`struct binder_thread`、`struct binder_buffer` 等 Binder 私有结构体字段。Android common 这些结构体不是 UAPI，字段布局会随内核分支变化，后续如果要解析字段，需要单独做版本适配。

## 控制面 IPC

当前控制面采用自定义 socket 协议族，不创建 `/dev` 节点。模块加载时从 `AF_DECnet` 开始动态寻找空闲协议族并注册，只允许 root 且具备 `CAP_SYS_ADMIN` 的进程创建 `SOCK_RAW` 控制 socket。

连接流程：

1. 用户态从 `AF_DECnet` 开始扫描协议族。
2. 先用 `SOCK_STREAM` 探测；如果内核返回 `ENOKEY`，说明该 family 像 binder-trace 控制协议族。
3. 再用同一个 family 创建 `SOCK_RAW`。
4. 通过 `BT_IOC_GET_FEATURE` 获取驱动特征，只有 `magic == BTRACE01` 才认为这是 binder-trace。
5. 特征匹配后再检查 ABI 版本并发送控制命令。

支持的控制命令定义在 `src/ipc/bt_ipc_uapi.h`：

- `BT_IOC_GET_ABI_VERSION`
- `BT_IOC_SET_CONFIG`
- `BT_IOC_GET_CONFIG`
- `BT_IOC_GET_STATS`
- `BT_IOC_CLEAR_STATS`
- `BT_IOC_GET_FEATURE`

自定义协议族同时承载低频控制面和当前阶段的事件流：ioctl 负责开启/关闭捕获、基础过滤和统计读取，`recvmsg`/`poll`/`epoll` 负责读取内核推送的固定布局 Binder 事件。事件结构当前为 ABI v2，包含当前任务身份、`binder_transaction()` 入口指针、transaction code、data/offsets 大小、目标 handle、发送方 pid/euid，以及发送方用户态 Parcel 的前 256 字节。模块只读取 Android Binder UAPI 里稳定的 `struct binder_transaction_data` 字段，不解引用 `struct binder_proc`、`struct binder_thread` 等 Binder 私有结构体。

## 版本来源

已经对照过的 Android common 源码：

- 5.10: https://android.googlesource.com/kernel/common/+/refs/heads/android12-5.10/drivers/android/binder.c
- 5.15: https://android.googlesource.com/kernel/common/+/refs/heads/android13-5.15/drivers/android/binder.c
- 6.1: https://android.googlesource.com/kernel/common/+/refs/heads/android14-6.1/drivers/android/binder.c
- 6.6: https://android.googlesource.com/kernel/common/+/refs/heads/android15-6.6/drivers/android/binder.c
- 6.12: https://android.googlesource.com/kernel/common/+/refs/heads/android16-6.12/drivers/android/binder.c

`binder_alloc_copy_user_to_buffer` 位于对应分支的 `drivers/android/binder_alloc.c`。5.10/5.15 内部使用 `kmap()`，6.1/6.6/6.12 内部使用 `kmap_local_page()`，但函数签名保持一致。

Android common 当前没有标准 `android*-5.19*` ACK/GKI 分支；如果后续遇到厂商 5.19 内核，需要拿厂商源码里的 `drivers/android/binder.c` 和 `drivers/android/binder_alloc.c` 单独对照。

## 构建

```bash
kernel/scripts/build-ddk.sh build android14-6.1
```

在 DDK devcontainer 里可以直接使用内核源码环境:

```bash
source kernel/envsetup.sh
make -C kernel
```

## 部署

```bash
kernel/scripts/insmod_ko.sh
```

当前阶段不支持普通 `rmmod` 热卸载。模块会自持有引用，避免 Binder 长阻塞
调用在卸载后返回到已经释放的模块文本段；需要替换模块时请重启测试设备。
