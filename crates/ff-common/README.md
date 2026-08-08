# ff-common

Shared buffer-pooling abstractions for the ff-* crate family.

Part of the [`avio`](https://github.com/itsakeyfut/avio) crate family.

## Overview

`ff-common` provides the `FramePool` trait, `PooledBuffer` type, and `VecPool` (a ready-to-use pool implementation) used internally across the `ff-*` crates. It has no external dependencies and does not link against FFmpeg.

`PooledBuffer` wraps an allocated block of memory and returns it to the originating pool automatically when dropped, so no manual free call is needed. If no pool is associated, the memory is deallocated. `FramePool` is `Send + Sync`, so pools can be shared across threads without additional locking.

## Usage

`ff-common` is an internal workspace crate, not intended for direct use in application code. The following example shows the `PooledBuffer::standalone` constructor, which allocates a buffer without a backing pool:

```rust
use ff_common::PooledBuffer;

// Wrap a 4096-byte buffer with no pool backing.
// Memory is freed normally when `buf` is dropped.
let buf = PooledBuffer::standalone(vec![0u8; 4096]);
assert_eq!(buf.len(), 4096);
```

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
