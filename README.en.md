# binder-trace

English | [中文](README.md)

`binder-trace` is an Android Binder call observability tool. It captures Binder transactions through a kernel module and provides WebUI, TUI, and JSONL output in user space. It is useful for investigating system service calls, interface frequency, payloads, and request/reply correlation.

## Screenshots

| WebUI                                                                                                                                                             | TUI                                                                                                                                                         |
|-------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------|
| [![binder-trace WebUI demo](https://github.com/fuqiuluo/binder-trace/raw/master/docs/assets/webui-demo-thumb.png)](https://github.com/fuqiuluo/binder-trace/raw/master/docs/assets/webui-demo.png) | [![binder-trace TUI demo](https://github.com/fuqiuluo/binder-trace/raw/master/docs/assets/tui-demo-thumb.png)](https://github.com/fuqiuluo/binder-trace/raw/master/docs/assets/tui-demo.png) |

## Features

- Capture Android Binder transactions in real time.
- WebUI: table view, search, direction/interface filters, details sidebar, payload/raw JSON, correlated calls, on-demand pagination, and resizable columns.
- TUI: real-time event list, frequency statistics, hexdump, and decoded details in the terminal.
- JSONL: stable event messages for scripts and later analysis.
- Narrow capture scope by `tgid`, `pid`, or `uid`.

## Prerequisites

- A rooted Android device.
- `adb` installed on the host machine.
- A `binder-trace` user-space binary matching the device ABI.
- A `bt-kmod.ko` kernel module matching the device kernel version.

First check the device information:

```bash
adb shell getprop ro.product.cpu.abi
adb shell uname -r
adb shell su -c id
```

## Install on device

Download `binder-trace` and the matching `bt-kmod.ko` kernel module from Releases, then push them to the device:

```bash
adb shell mkdir -p /data/local/tmp/binder-trace
adb push binder-trace /data/local/tmp/binder-trace/binder-trace
adb push bt-kmod.ko /data/local/tmp/binder-trace/bt-kmod.ko
adb shell chmod 755 /data/local/tmp/binder-trace/binder-trace
```

Load the kernel module:

```bash
adb shell su -c 'insmod /data/local/tmp/binder-trace/bt-kmod.ko'
```

Confirm that the module is available:

```bash
adb shell su -c 'lsmod | grep bt_kmod'
adb shell su -c '/data/local/tmp/binder-trace/binder-trace ipc feature'
```

If an older module is already loaded on the device, unload it first:

```bash
adb shell su -c 'rmmod bt_kmod'
```

`rmmod` waits for threads already inside Binder hooks to exit. If the device has long-blocking Binder read/looper threads, unloading may take some time.

## Use WebUI

WebUI is the recommended daily inspection mode. First forward the port:

```bash
adb forward tcp:5173 tcp:5173
```

Start WebUI on the device:

```bash
adb shell
su
cd /data/local/tmp/binder-trace
./binder-trace webui --listen 127.0.0.1:5173
```

Then open this URL in a browser on the host machine:

```text
http://127.0.0.1:5173/
```

Common options:

```bash
./binder-trace webui --uid 1000
./binder-trace webui --tgid 12345
./binder-trace webui --pid 12345
./binder-trace webui --no-enable
./binder-trace webui --android-sdk 35
./binder-trace webui --history-path /data/local/tmp/binder-trace/webui-events.btcap
./binder-trace webui --max-history-bytes 8589934592
```

Notes:

- Binder transaction capture is enabled by default.
- `--no-enable` only reads the existing event stream and does not modify the kernel capture configuration.
- WebUI events are written to the shared `CaptureHistory` btcap backend. The default file is `/data/local/tmp/binder-trace/webui-events.btcap`, with a default maximum accumulated file space of 8 GiB.
- WebUI filtering and pagination run on the backend. The browser only renders the current window, and the backend does not discard captured events because of the browser window size.
- The bottom-right control can switch the current render window size: `256`, `1024`, or `4096`.

## Use TUI

TUI is useful for quick event inspection in the terminal. An interactive shell is recommended to avoid non-interactive `adb shell su -c` behavior affecting key input and terminal size:

```bash
adb shell
su
cd /data/local/tmp/binder-trace
./binder-trace tui
```

Common options:

```bash
./binder-trace tui --rows 1024 --refresh-ms 100
./binder-trace tui --uid 1000
./binder-trace tui --tgid 12345
./binder-trace tui --pid 12345
./binder-trace tui --no-enable
./binder-trace tui --history-path /data/local/tmp/binder-trace/events.btcap
```

Default TUI history files:

- Android device: `/data/local/tmp/binder-trace/events.btcap`
- Other environments: `binder-trace.btcap`

The interface language is selected from the Android locale or the `LANG` / `LC_*` environment variables. Built-in languages currently include English, Chinese, and Japanese.

## Use MCP

The MCP entry point lets an AI assistant query Binder traces online. It provides a `/mcp` endpoint through Streamable HTTP. By default, it only reads the real-time event stream and does not modify the kernel capture configuration.

First forward the port:

```bash
adb forward tcp:5174 tcp:5174
```

Start the MCP service on the device:

```bash
adb shell
su
cd /data/local/tmp/binder-trace
./binder-trace mcp --listen 127.0.0.1:5174
```

If the AI assistant needs permission to enable or disable Binder trace, explicitly allow control at startup:

```bash
./binder-trace mcp --listen 127.0.0.1:5174 --allow-control
```

Capture can also be enabled immediately when the MCP service starts:

```bash
./binder-trace mcp --listen 127.0.0.1:5174 --allow-control --enable --uid 1000
```

To specify the MCP history file, initial capacity, or file size limit:

```bash
./binder-trace mcp --history-path /data/local/tmp/binder-trace/custom-mcp.btcap --rows 65536
./binder-trace mcp --max-history-bytes 8589934592
```

MCP history is written to `/data/local/tmp/binder-trace/mcp-events.btcap` by default, with a default maximum accumulated file space of 8 GiB. After the limit is reached, the service refuses further writes and disables kernel capture on a best-effort basis to avoid continuing to fill the audit service with abnormal traffic.

When using a desktop MCP client, connect to the forwarded local HTTP address. Different clients use slightly different field names; the important pieces are Streamable HTTP and a URL ending in `/mcp`:

```json
{
  "mcpServers": {
    "binder-trace": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:5174/mcp"
    }
  }
}
```

This setup does not automatically start `binder-trace` through `adb shell` when the MCP host starts. The client can connect only after the service is started manually on the device.

Current MCP tools:

- `binder_trace_status`: inspect driver features, capture configuration, statistics, and btcap history status.
- `binder_trace_enable`: enable Binder transaction capture, with optional `tgid`, `pid`, `uid`, `min_size`, and `max_size`.
- `binder_trace_disable`: disable kernel capture.
- `binder_trace_clear_stats`: clear kernel statistics.
- `binder_trace_events`: query events with direction, interface, text, `after_seq`/`before_seq` pagination, and `since_ns`/`until_ns` time-window filters.
- `binder_trace_event`: query one event by sequence.
- `binder_trace_top_interfaces`: count the most active Binder interfaces in the current btcap history.
- `binder_trace_clear_history`: clear MCP btcap history without changing kernel state.

## JSONL output

When run without a subcommand, `binder-trace` connects to the socket event stream provided by
the kernel module, enables Binder transaction capture, and continuously writes requests and
replies to JSONL for scripting or saving:

```bash
adb shell
su
cd /data/local/tmp/binder-trace
./binder-trace --output trace.jsonl
```

The current ABI cannot distinguish between `binder`, `hwbinder`, and `vndbinder`, so
`binder_device` is always `unknown`. This field is independent of the `/dev/binder` path, and
using binderfs does not affect event capture.

The outer event object is a common message envelope. `object` indicates the event type, and `data` contains the corresponding payload:

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

## Build from source

Run project checks:

```bash
cargo run -p xtask -- check
cargo test --workspace
```

Build and push the Android user-space binary:

```bash
export ANDROID_NDK_HOME=/path/to/android-ndk
android/push.sh
```

`android/push.sh` builds an `aarch64-linux-android` debug binary by default and pushes it to `/data/local/tmp/binder-trace/binder-trace`. The following environment variables can adjust its behavior:

- `BINDER_TRACE_ANDROID_TARGET`
- `BINDER_TRACE_ANDROID_API`
- `BINDER_TRACE_PROFILE`
- `BINDER_TRACE_REMOTE_DIR`
- `BINDER_TRACE_BIN`
- `BINDER_TRACE_DEVICE_ID`

Build the Android kernel module:

```bash
kernel/scripts/build-ddk.sh build android14-6.1
```

Clean kernel module build artifacts:

```bash
kernel/scripts/build-ddk.sh clean android14-6.1
```

## Repository layout

- `kernel/`: Android kernel module, hooks, UAPI, and DDK build scripts.
- `android/`: adb push and device-run helper scripts.
- `crates/bt-common/`: fixed-layout types shared by kernel space and user space.
- `crates/bt-agent/`: user-space event reading, control protocol, and diagnostics.
- `crates/bt-decoder/`: Binder event decoding and Android platform method tables.
- `crates/bt-storage/`: JSONL persistence.
- `crates/bt-cli/`: `binder-trace` command-line entry point and TUI.
- `crates/bt-webui/`: embedded WebUI.
- `xtask/`: local developer command wrapper.
- `docs/`: screenshots and development docs.

## Notes

- This project is intended only for debugging, security research, and compatibility analysis on Android devices that you own or are explicitly authorized to test. Do not use it for unauthorized access, covert monitoring, audit evasion, extracting third-party data, or any unlawful purpose.
- Binder payloads may contain personal information, account data, communications content, or business data. Before collecting, storing, transferring, or sharing traces, make sure you have lawful authorization and limit collection scope, retention time, and access permissions to what is necessary.
- When `binder-trace` enters a real execution path, it writes `/data/local/tmp/.fuqiuluo` on a best-effort basis to mark that the tool has run. `--help` and argument parsing failures do not write this marker.
- To inspect capture-side debug logs, set `RUST_LOG`:

```bash
RUST_LOG=bt_agent=debug android/run-root.sh webui
RUST_LOG=bt_agent=trace android/run-root.sh tui
```

## Acknowledgements

Thanks to the [foundryzero/binder-trace](https://github.com/foundryzero/binder-trace) project.

## License

- User-space Rust crates, `xtask`, Android helper scripts, documentation, and other non-kernel code: `MIT OR Apache-2.0`.
- Android/Linux kernel module under `kernel/`: `GPL-2.0-only`.
- UAPI headers required by user space follow the SPDX identifiers in each file, for example `kernel/src/ipc/bt_ipc_uapi.h`: `(GPL-2.0-only WITH Linux-syscall-note) OR MIT`.

See the root [`LICENSE`](LICENSE) file for the full terms.
