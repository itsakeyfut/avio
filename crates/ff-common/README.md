# ff-common

Shared buffer-pooling abstractions for the ff-* crate family.

`ff-common` is a small, FFmpeg-free foundation shared across the `ff-*` family: frame-buffer memory pooling (`FramePool`, `PooledBuffer`, `VecPool`). It links no FFmpeg and contains no `unsafe`. Applications usually get it transitively through the decode/encode crates, though `VecPool` is directly usable to control frame allocation.

## Overview

`ff-common` provides the `FramePool` trait, `PooledBuffer` type, and `VecPool` (a ready-to-use pool implementation) used internally across the `ff-*` crates. It has no external dependencies and does not link against FFmpeg.

`PooledBuffer` wraps an allocated block of memory and returns it to the originating pool automatically when dropped, so no manual free call is needed. If no pool is associated, the memory is deallocated. `FramePool` is `Send + Sync`, so pools can be shared across threads without additional locking.

## Usage

`ff-common` is an internal workspace crate, not intended for direct use in application code. The following program shows `VecPool`, the ready-to-use pool: buffers released to it are reused by later `acquire` calls, and a `PooledBuffer` returns itself to its pool automatically when dropped.

```rust
use ff_common::{FramePool, VecPool};

fn main() {
    // A pool that retains up to 4 reusable buffers. `VecPool::new`
    // returns an `Arc<VecPool>` so it can be shared across threads.
    let pool = VecPool::new(4);
    assert_eq!(pool.available(), 0); // starts empty

    // Seed the pool with one buffer, then acquire it back out.
    pool.release(vec![0u8; 2048]);
    assert_eq!(pool.available(), 1);

    {
        // `acquire` hands back the smallest buffer that fits, resized to
        // the requested length. When `buf` is dropped it returns to `pool`.
        let buf = pool.acquire(1024).unwrap();
        assert_eq!(buf.len(), 1024);
        assert_eq!(pool.available(), 0);
    }

    // The buffer was automatically returned on drop.
    assert_eq!(pool.available(), 1);
    println!("pool now holds {} buffer(s)", pool.available());
}
```

For a buffer with no backing pool, `PooledBuffer::standalone` wraps an existing `Vec<u8>`; its memory is simply freed when dropped:

```rust
use ff_common::PooledBuffer;

let buf = PooledBuffer::standalone(vec![0u8; 4096]);
assert_eq!(buf.len(), 4096);
```

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
