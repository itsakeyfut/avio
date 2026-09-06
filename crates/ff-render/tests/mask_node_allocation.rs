//! A mask node must not allocate per frame (#1710).
//!
//! `LumaMask` and `ShapeMask` used to carry a full-frame mask buffer: the compositor
//! built one `Vec<u8>` per graph rebuild and the GPU path uploaded a whole texture per
//! frame on top of that. Both masks are computed in the shader now, so the steady state
//! is a node that holds a rectangle and a flag and touches no allocator at all.
//!
//! Counted on the CPU path deliberately: it is the same node object the GPU path drives,
//! it is deterministic, and it needs no adapter. The texture upload cannot be counted
//! from Rust; its removal is visible in the diff instead.
//!
//! Counting is **per thread**, so the suite stays correct at default parallelism: another
//! test's allocations are invisible to this one (RK-019).
//!
//! The control below is what keeps this non-vacuous: an allocator that silently stopped
//! counting would let a leaking node pass.

// A `GlobalAlloc` cannot be implemented without `unsafe`, and there is no safe way to
// observe the allocator. Test-only, and outside `src/`, so the confinement rule that
// keeps `unsafe` in `*_inner` modules is untouched.
#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use ff_render::{LumaMaskNode, RenderNodeCpu, ShapeMaskNode};

thread_local! {
    /// `Some(n)` while this thread is counting. `const`-initialised so the allocator
    /// never allocates to reach it.
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Counts this thread's allocations while counting is on, and forwards to `System`.
struct CountingAllocator;

// SAFETY: every call is forwarded verbatim to the system allocator; the counter only
// observes, and reaching it cannot itself allocate (`try_with` on a `const` thread-local
// `Cell`, so a thread whose TLS is being torn down is simply not counted).
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn bump() {
    let _ = COUNT.try_with(|c| {
        if let Some(n) = c.get() {
            c.set(Some(n + 1));
        }
    });
}

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

const W: u32 = 320;
const H: u32 = 180;
const FRAMES: usize = 30;

/// Runs `f` with counting on and returns how many allocations it made.
fn count<F: FnMut()>(mut f: F) -> usize {
    COUNT.with(|c| c.set(Some(0)));
    f();
    COUNT.with(|c| c.replace(None)).unwrap_or(0)
}

fn frame() -> Vec<u8> {
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let v = (i % 256) as u8;
        *px = [v, v, v, 255];
    }
    rgba
}

#[test]
fn the_allocation_counter_should_observe_a_real_allocation() {
    // The control (RK-015): the assertions below only mean something if a `Vec` the size
    // of the old mask buffer is actually counted.
    let n = count(|| {
        let buf = vec![0u8; (W * H * 4) as usize];
        std::hint::black_box(&buf);
    });
    assert!(n > 0, "the counter must observe a frame-sized Vec, got {n}");
}

#[test]
fn a_shape_mask_node_should_not_allocate_per_frame() {
    let node = ShapeMaskNode::new(20, 10, 100, 60, false);
    let mut rgba = frame();
    // One warm-up outside the count: nothing is built lazily on this path today, and a
    // future one-off would still leave the steady state at zero.
    node.process_cpu(&mut rgba, W, H);

    let n = count(|| {
        for _ in 0..FRAMES {
            node.process_cpu(&mut rgba, W, H);
        }
    });
    assert_eq!(n, 0, "{FRAMES} frames of shape masking allocated {n} times");
}

#[test]
fn a_luma_mask_node_should_not_allocate_per_frame() {
    let node = LumaMaskNode::new(false);
    let mut rgba = frame();
    node.process_cpu(&mut rgba, W, H);

    let n = count(|| {
        for _ in 0..FRAMES {
            node.process_cpu(&mut rgba, W, H);
        }
    });
    assert_eq!(n, 0, "{FRAMES} frames of luma masking allocated {n} times");
}

#[cfg(feature = "wgpu")]
#[test]
fn an_animated_rectangle_should_not_allocate_per_frame() {
    // The moving-rectangle case the issue is about: the rectangle changes every frame,
    // and taking the new one must stay a `Cell` write rather than a graph rebuild.
    use ff_render::{NodeParam, RenderNode};

    let node = ShapeMaskNode::new(0, 0, 100, 60, false);
    let mut rgba = frame();
    node.process_cpu(&mut rgba, W, H);

    let mut accepted = 0usize;
    let n = count(|| {
        for i in 0..FRAMES {
            if node.set_param(NodeParam::ShapeMaskRect {
                x: i as u32,
                y: 10,
                width: 100,
                height: 60,
                invert: false,
            }) {
                accepted += 1;
            }
            node.process_cpu(&mut rgba, W, H);
        }
    });
    assert_eq!(accepted, FRAMES, "the node must take its own rectangle");
    assert_eq!(n, 0, "an animated rectangle allocated {n} times");
}
