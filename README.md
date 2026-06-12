# binder-trace

Binder trace tooling workspace.

## Layout

- `xtask/`: project entrypoint for local tasks and developer commands.
- `crates/bt-common/`: shared eBPF/userspace data structures. Keep this crate small and C-layout friendly.
- `crates/bt-ebpf/`: kernel-side capture layer.
- `crates/bt-agent/`: userspace core process.
- `crates/bt-decoder/`: decoder for raw eBPF event streams.
- `crates/bt-storage/`: JSONL storage layer, with room for SQLite later.
- `crates/bt-cli/`: debugging CLI, intended to grow toward a stackplz-like workflow.
- `crates/bt-web/`: future web UI/service surface.

## Commands

```bash
cargo run -p xtask -- check
cargo run -p xtask -- run --output trace.jsonl
cargo run -p bt-cli -- --output trace.jsonl
```

## Development

Project-wide documentation and code requirements are tracked in
[`docs/development-guidelines.md`](docs/development-guidelines.md).
