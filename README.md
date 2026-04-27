# vm-agent

Rust VM execution agent.

## Structure

- `configs/`: runtime config profiles (`default`, `dev`, `prod`)
- `proto/`: gRPC contracts (`vm`, `task`, `health`)
- `src/app`: bootstrap, lifecycle, app context/state
- `src/config`: typed config modules
- `src/model`: domain models/enums shared in agent
- `src/service`: business and reconcile flows
- `src/runtime`: drivers (`qemu`, `libvirt`, process bridge)
- `src/storage`: local disk/volume metadata handling
- `src/network`: bridge/tap/ip operations
- `src/worker`: background worker pool and dispatch
- `src/transport`: gRPC/HTTP transport adapters
- `src/repository`: metadata persistence boundary
- `src/queue`: queue adapters (e.g. Redis)
- `src/telemetry`: logging/metrics
- `src/error`: app error types
- `src/common`: shared helpers
- `tests/`: integration and fixture modules

## Getting started

1. Copy `.env.example` to `.env` and adjust values.
2. Pick `configs/dev.toml` for local run.
3. Implement drivers and transport contracts incrementally.
