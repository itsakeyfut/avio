# Development Rules

This directory collects the conventions to follow when writing code across the avio workspace.

| File | Contents |
|---|---|
| [rust.md](./rust.md) | Formatting, naming, module layout, concurrency, code quality |
| [design.md](./design.md) | Engine vs primitive boundary, encapsulation, crate boundaries |
| [error-handling.md](./error-handling.md) | Error-type design, `Result` vs panic, `thiserror` |
| [logging.md](./logging.md) | Log levels, message format, hot-path policy |
| [unsafe.md](./unsafe.md) | `unsafe` isolation, SAFETY comments, FFmpeg pointer/ownership |
| [perf.md](./perf.md) | Hot-path allocation, buffer pooling, benchmarks |
| [test.md](./test.md) | Test naming, fixtures, probe-gating, integration-test policy |
| [gpu.md](./gpu.md) | wgpu resource lifecycle, bytemuck, color (`ff-render`) |
| [wgsl.md](./wgsl.md) | Shader naming, struct alignment, bindings (`ff-render`) |

Design specs live in [`../specs/`](../specs/). Internal review-knowledge and process docs live in
`docs/dev/`.
