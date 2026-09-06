//! RAII owner for a custom `AVIOContext` backed by a Rust [`IoSource`] or
//! [`IoSink`].
//!
//! [`IoContext`] allocates the context and its buffer, keeps the boxed Rust
//! reader/writer alive for exactly as long as FFmpeg may call back into it, and
//! frees all three exactly once on drop. It is `pub(crate)`: callers reach it
//! through [`InputFormatContext::open_custom`](crate::InputFormatContext::open_custom)
//! and [`OutputFormatContext::set_custom_io`](crate::OutputFormatContext::set_custom_io),
//! so no public signature mentions an `AVIOContext` at all.
//!
//! # Ownership contract
//!
//! From `libavformat/avio.h`, which is the authority here rather than any prose
//! elsewhere:
//!
//! - the buffer must come from `av_malloc`;
//! - libavformat **may free it and install a different one**, so the buffer to
//!   release on teardown is whatever `AVIOContext.buffer` points at then, not the
//!   pointer originally passed in;
//! - the context itself is released by `avio_context_free`, which does not touch
//!   the buffer.
//!
//! Hence the drop order: free `ctx->buffer`, free the context, then drop the
//! boxed Rust state.

use std::ffi::c_void;
use std::io::{ErrorKind, SeekFrom};
use std::os::raw::{c_int, c_uchar};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

use crate::io_traits::{IoSink, IoSource};
use crate::{AVIOContext, AvError, av_freep as ffi_av_freep, av_malloc as ffi_av_malloc};

// Declared here rather than taken from bindgen: the generator's allowlist covers
// `av_*` / `avformat_*` / `avcodec_*` and friends, none of which match `avio_*`,
// so no `avio_` function reaches the generated bindings. This mirrors the
// existing `avio_open` / `avio_closep` declarations in `avformat.rs`.
//
// The three callback types are pinned to the ones `AVIOContext` itself declares
// by `assert_callback_abi` below.
unsafe extern "C" {
    fn avio_alloc_context(
        buffer: *mut c_uchar,
        buffer_size: c_int,
        write_flag: c_int,
        opaque: *mut c_void,
        read_packet: Option<unsafe extern "C" fn(*mut c_void, *mut u8, c_int) -> c_int>,
        write_packet: Option<unsafe extern "C" fn(*mut c_void, *const u8, c_int) -> c_int>,
        seek: Option<unsafe extern "C" fn(*mut c_void, i64, c_int) -> i64>,
    ) -> *mut AVIOContext;
    fn avio_context_free(s: *mut *mut AVIOContext);
}

/// Fails to compile if the hand-declared callback types stop matching the ones
/// bindgen generated for `AVIOContext`.
///
/// A hand-written FFI signature that disagrees with the C ABI produces no
/// diagnostic and no test failure -- it corrupts the stack at the call. Neither a
/// Windows development machine nor the `DOCS_RS` job (which compiles against
/// stubs) would show it, so the check has to be a compile-time one, here, against
/// the generated struct.
#[allow(dead_code)]
fn assert_callback_abi(ctx: &AVIOContext) {
    let _: Option<unsafe extern "C" fn(*mut c_void, *mut u8, c_int) -> c_int> = ctx.read_packet;
    let _: Option<unsafe extern "C" fn(*mut c_void, *const u8, c_int) -> c_int> = ctx.write_packet;
    let _: Option<unsafe extern "C" fn(*mut c_void, i64, c_int) -> i64> = ctx.seek;
}

/// Buffer size handed to `avio_alloc_context`.
///
/// `avio.h` recommends a protocol's block size, or a cache page for everything
/// else. There is no block size to match here, so it is a page.
const BUFFER_SIZE: usize = 4096;

/// `AVSEEK_SIZE`: the demuxer is asking for the stream's total length rather than
/// seeking. Present in the bindings; named here for readability alongside
/// [`AVSEEK_FORCE`].
const AVSEEK_SIZE: c_int = crate::AVSEEK_SIZE as c_int;

/// `AVSEEK_FORCE`: a hint OR-ed into `whence`, not a position. It has to be
/// masked off before the remaining bits are read as a `SeekFrom`.
const AVSEEK_FORCE: c_int = 0x2_0000;

/// The Rust end of the callbacks, owned by an [`IoContext`].
enum IoState {
    Read(Box<dyn IoSource>),
    Write(Box<dyn IoSink>),
}

/// An owned custom `AVIOContext` and the Rust reader or writer behind it.
pub(crate) struct IoContext {
    ptr: NonNull<AVIOContext>,
    /// The boxed state, held as a raw pointer rather than a live `Box` because
    /// FFmpeg holds the same address in `opaque` for as long as this type lives.
    /// Keeping it raw means no Rust reference to it exists between callbacks.
    state: NonNull<IoState>,
}

impl IoContext {
    /// Wraps `source` as a readable `AVIOContext`.
    ///
    /// # Errors
    ///
    /// Returns `ENOMEM` if the buffer or the context cannot be allocated.
    pub(crate) fn reader(source: impl IoSource + 'static) -> Result<Self, AvError> {
        Self::alloc(IoState::Read(Box::new(source)), 0)
    }

    /// Wraps `sink` as a writable `AVIOContext`.
    ///
    /// # Errors
    ///
    /// Returns `ENOMEM` if the buffer or the context cannot be allocated.
    pub(crate) fn writer(sink: impl IoSink + 'static) -> Result<Self, AvError> {
        Self::alloc(IoState::Write(Box::new(sink)), 1)
    }

    /// The raw context, for attaching to a format context's `pb`.
    pub(crate) fn as_ptr(&self) -> *mut AVIOContext {
        self.ptr.as_ptr()
    }

    /// Allocates the buffer and the context around an already-boxed state.
    fn alloc(state: IoState, write_flag: c_int) -> Result<Self, AvError> {
        crate::ensure_initialized();

        // SAFETY: a plain allocation; the size is a compile-time constant. FFmpeg
        //         requires the buffer to come from `av_malloc` specifically, so
        //         this cannot be a Rust allocation.
        let buffer = unsafe { alloc_buffer() };
        let Some(buffer) = NonNull::new(buffer) else {
            return Err(AvError::new(crate::error_codes::ENOMEM));
        };

        let state = NonNull::from(Box::leak(Box::new(state)));
        let (read, write) = match write_flag {
            0 => (
                Some(read_packet as unsafe extern "C" fn(*mut c_void, *mut u8, c_int) -> c_int),
                None,
            ),
            _ => (
                None,
                Some(write_packet as unsafe extern "C" fn(*mut c_void, *const u8, c_int) -> c_int),
            ),
        };

        // SAFETY: `buffer` is a live `av_malloc` allocation of `BUFFER_SIZE`, which
        //         FFmpeg takes over. `state` is a leaked box, so the address stays
        //         valid until this type's `Drop` reclaims it, which is strictly
        //         after the context is freed and therefore after the last callback.
        //         The callback signatures match `AVIOContext`'s own (checked by
        //         `assert_callback_abi`).
        let ptr = unsafe {
            avio_alloc_context(
                buffer.as_ptr(),
                c_int::try_from(BUFFER_SIZE).unwrap_or(c_int::MAX),
                write_flag,
                state.as_ptr().cast::<c_void>(),
                read,
                write,
                Some(seek),
            )
        };

        match NonNull::new(ptr) {
            Some(ptr) => Ok(Self { ptr, state }),
            None => {
                // The context was not created, so nothing took ownership of either
                // allocation: release both here rather than leaking on the error
                // path.
                // SAFETY: `buffer` is still the `av_malloc` allocation nothing else
                //         holds, and `state` is the box just leaked above.
                unsafe {
                    let mut raw = buffer.as_ptr().cast::<c_void>();
                    ffi_av_freep(std::ptr::addr_of_mut!(raw).cast::<c_void>());
                    drop(Box::from_raw(state.as_ptr()));
                }
                Err(AvError::new(crate::error_codes::ENOMEM))
            }
        }
    }
}

/// `av_malloc(BUFFER_SIZE)` as a `*mut c_uchar`.
///
/// Deliberately *not* zeroing: `read_packet` zeroes what it is about to hand a
/// `Read` impl anyway, and that has to cover buffers libavformat swaps in later,
/// which this never sees.
///
/// # Safety
///
/// Always safe to call; the result must be released with `av_free` / `av_freep`
/// unless ownership passes to FFmpeg.
unsafe fn alloc_buffer() -> *mut c_uchar {
    // SAFETY: `av_malloc` accepts any size and returns null on failure, which the
    //         caller checks.
    unsafe { ffi_av_malloc(BUFFER_SIZE).cast::<c_uchar>() }
}

impl std::fmt::Debug for IoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The source/sink is caller-supplied and carries no `Debug` bound, and
        // reading the direction would mean dereferencing `state` from a formatter.
        // The owning format contexts derive `Debug`, so this only has to exist.
        f.debug_struct("IoContext").finish_non_exhaustive()
    }
}

impl Drop for IoContext {
    fn drop(&mut self) {
        // No `avio_flush` here. It looked necessary -- `avio_context_free` discards
        // whatever is still buffered, and `avio.h` does not say `av_write_trailer`
        // flushes the `pb` -- but it is not: with the flush removed, muxing a
        // 9923-byte stream through the 4 KiB buffer produced byte-identical output
        // (300 frames, sink and file routes alike), so the trailer path already
        // drains it. Re-add it only with a case that shows a difference.
        //
        // SAFETY: we uniquely own the context (NonNull, not Copy/Clone), so this
        //         runs exactly once. The buffer freed is `ctx->buffer` rather than
        //         the pointer originally passed in, because libavformat may have
        //         replaced it (`avio.h`); `avio_context_free` does not free it. The
        //         boxed state is reclaimed last, after the context that could still
        //         have called into it is gone.
        unsafe {
            let ctx = self.ptr.as_ptr();
            ffi_av_freep(std::ptr::addr_of_mut!((*ctx).buffer).cast::<c_void>());
            let mut raw = ctx;
            avio_context_free(std::ptr::addr_of_mut!(raw));
            drop(Box::from_raw(self.state.as_ptr()));
        }
    }
}

// SAFETY: an `AVIOContext` is not safe for concurrent access, but moving
//         ownership between threads is sound because Rust's ownership model
//         guarantees exclusive access, and the boxed source/sink is itself
//         `Send` by the trait bound.
unsafe impl Send for IoContext {}

/// Runs `f` with the state behind `opaque`, mapping a panic to `EIO`.
///
/// # Safety
///
/// `opaque` must be null or the `IoState` pointer an [`IoContext`] leaked for
/// this context, still alive because `Drop` reclaims it only after the context is
/// freed.
///
/// Every callback goes through here. A panic that reached the `extern "C"`
/// boundary would abort the process, so it is caught and reported as an I/O
/// error instead -- FFmpeg then fails the read or write the way it would for any
/// other failing source.
unsafe fn with_state<T>(opaque: *mut c_void, err: T, f: impl FnOnce(&mut IoState) -> T) -> T {
    if opaque.is_null() {
        return err;
    }
    // SAFETY: `opaque` is the leaked `IoState` box this context was built with,
    //         still alive because `Drop` reclaims it only after the context is
    //         freed. FFmpeg calls back serially on the thread that owns the
    //         context, so this is the only reference in existence.
    let state = unsafe { &mut *opaque.cast::<IoState>() };
    catch_unwind(AssertUnwindSafe(|| f(state))).unwrap_or(err)
}

/// `read_packet`: fill `buf` from the Rust source.
///
/// Returns `AVERROR_EOF` rather than 0 at end of stream, which `avio.h` requires.
///
/// # Safety
///
/// `opaque` must be the `IoState` pointer this context was built with, and `buf`
/// must point at `buf_size` writable bytes. FFmpeg upholds both for the callbacks
/// it was handed.
unsafe extern "C" fn read_packet(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let eio = crate::error_codes::EIO;
    if buf.is_null() || buf_size <= 0 {
        return crate::error_codes::EINVAL;
    }
    let len = buf_size as usize;
    // SAFETY: `buf` is writable for `len` (FFmpeg's contract). Zeroing first is not
    //         defensive padding: the buffer came from `av_malloc`, and any replacement
    //         libavformat installs is `av_malloc`/`av_realloc` too, so it is
    //         uninitialized. `Read::read` is safe code that is permitted to read from
    //         the slice it is handed, and a `&mut [u8]` over uninitialized memory is
    //         not a valid reference. The memset is one page per refill, against an
    //         actual I/O.
    unsafe { std::ptr::write_bytes(buf, 0, len) };
    // SAFETY: `buf` is writable for `len` and now initialized.
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    // SAFETY: `opaque` is this context's state; see `with_state`.
    unsafe {
        with_state(opaque, eio, |state| {
            let IoState::Read(source) = state else {
                return eio;
            };
            loop {
                match source.read(out) {
                    Ok(0) => return crate::error_codes::EOF,
                    // `Read` is a safe trait: an implementation may report more bytes
                    // than it was given without any `unsafe` of its own. Passing that
                    // on would move FFmpeg's `buf_end` past the allocation
                    // (`fill_buffer` does `buf_end = dst + len`), so it is rejected
                    // here rather than trusted.
                    Ok(n) if n > len => return eio,
                    Ok(n) => return c_int::try_from(n).unwrap_or(eio),
                    // Retry rather than report zero bytes: `avio.h` says this callback
                    // "must never return 0 but rather a proper AVERROR code", and
                    // `read_packet_wrapper` asserts it. Retrying is what `Read::read_exact`
                    // does with the same error.
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => return eio,
                }
            }
        })
    }
}

/// `write_packet`: drain `buf` into the Rust sink.
///
/// # Safety
///
/// `opaque` must be the `IoState` pointer this context was built with, and `buf`
/// must point at `buf_size` readable bytes that stay unchanged for the call.
unsafe extern "C" fn write_packet(opaque: *mut c_void, buf: *const u8, buf_size: c_int) -> c_int {
    let eio = crate::error_codes::EIO;
    if buf.is_null() || buf_size < 0 {
        return crate::error_codes::EINVAL;
    }
    // SAFETY: FFmpeg guarantees `buf` points at `buf_size` readable bytes and does
    //         not change them for the duration of the call.
    let data = unsafe { std::slice::from_raw_parts(buf, buf_size as usize) };
    // SAFETY: `opaque` is this context's state; see `with_state`.
    unsafe {
        with_state(opaque, eio, |state| {
            let IoState::Write(sink) = state else {
                return eio;
            };
            match sink.write_all(data) {
                Ok(()) => buf_size,
                Err(_) => eio,
            }
        })
    }
}

/// `seek`: reposition the Rust source or sink, or answer `AVSEEK_SIZE`.
///
/// # Safety
///
/// `opaque` must be the `IoState` pointer this context was built with.
unsafe extern "C" fn seek(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    let eio = i64::from(crate::error_codes::EIO);
    // SAFETY: `opaque` is this context's state; see `with_state`.
    unsafe {
        with_state(opaque, eio, |state| {
            // `AVSEEK_FORCE` is a hint OR-ed onto `whence`, not a position.
            let whence = whence & !AVSEEK_FORCE;
            if whence == AVSEEK_SIZE {
                return stream_len(state).map_or(eio, |len| len);
            }
            let pos = match whence {
                // A negative absolute position is not a seek that can be performed.
                // Answering it with 0 would be a *different*, valid seek.
                0 => match u64::try_from(offset) {
                    Ok(pos) => SeekFrom::Start(pos),
                    Err(_) => return i64::from(crate::error_codes::EINVAL),
                },
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => return i64::from(crate::error_codes::EINVAL),
            };
            let seeked = match state {
                IoState::Read(source) => source.seek(pos),
                IoState::Write(sink) => sink.seek(pos),
            };
            seeked.map_or(eio, |p| i64::try_from(p).unwrap_or(eio))
        })
    }
}

/// The total length of the stream, leaving the position where it was.
///
/// Answering `AVSEEK_SIZE` rather than failing it lets a demuxer size the
/// container up front instead of probing towards the end.
fn stream_len(state: &mut IoState) -> Option<i64> {
    fn measure(
        current: std::io::Result<u64>,
        mut seek: impl FnMut(SeekFrom) -> std::io::Result<u64>,
    ) -> Option<i64> {
        let here = current.ok()?;
        let end = seek(SeekFrom::End(0)).ok()?;
        seek(SeekFrom::Start(here)).ok()?;
        i64::try_from(end).ok()
    }
    match state {
        IoState::Read(source) => {
            let here = source.stream_position();
            measure(here, |p| source.seek(p))
        }
        IoState::Write(sink) => {
            let here = sink.stream_position();
            measure(here, |p| sink.seek(p))
        }
    }
}

#[cfg(test)]
mod tests {
    //! What these tests cannot cover.
    //!
    //! `Drop` frees `ctx->buffer` **before** `avio_context_free`, because
    //! libavformat may have replaced the buffer and the context free does not
    //! touch it. Swapping those two lines is a use-after-free that no assertion
    //! here detects: reading freed memory does not reliably fault, and `miri`
    //! cannot run the FFI. The same is true of the explicit
    //! `AVFMT_FLAG_CUSTOM_IO` set on the input side, which this FFmpeg happens to
    //! perform itself. Both were confirmed uncaught by mutation injection. Their
    //! justification is the contract in `avio.h` / `avformat.h`, not a green test,
    //! and a passing suite here should not be read as covering them.

    use std::io::Cursor;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{AVSEEK_FORCE, AVSEEK_SIZE, IoContext, read_packet, seek, write_packet};

    /// A cursor that records its own drop, so "freed exactly once" is a count and
    /// not a judgement.
    struct CountingSource {
        inner: Cursor<Vec<u8>>,
        drops: Arc<AtomicUsize>,
    }

    impl std::io::Read for CountingSource {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }
    }

    impl std::io::Seek for CountingSource {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    impl Drop for CountingSource {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A source whose every read panics, to prove the panic never reaches FFmpeg.
    struct PanickingSource;

    impl std::io::Read for PanickingSource {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            panic!("the source panicked");
        }
    }

    impl std::io::Seek for PanickingSource {
        fn seek(&mut self, _pos: std::io::SeekFrom) -> std::io::Result<u64> {
            Ok(0)
        }
    }

    #[test]
    fn dropping_the_context_should_drop_the_source_exactly_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        let source = CountingSource {
            inner: Cursor::new(vec![1u8, 2, 3, 4]),
            drops: Arc::clone(&drops),
        };
        let ctx = IoContext::reader(source).expect("allocation should succeed");
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "still owned by the context"
        );
        drop(ctx);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "the source must be dropped exactly once with the context"
        );
    }

    #[test]
    fn dropping_a_writer_context_should_drop_the_sink_exactly_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        struct CountingSink(Arc<AtomicUsize>, Cursor<Vec<u8>>);
        impl std::io::Write for CountingSink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.1.write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl std::io::Seek for CountingSink {
            fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
                self.1.seek(pos)
            }
        }
        impl Drop for CountingSink {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let ctx = IoContext::writer(CountingSink(Arc::clone(&drops), Cursor::new(Vec::new())))
            .expect("allocation should succeed");
        drop(ctx);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn read_packet_should_report_eof_rather_than_zero() {
        // `avio.h` requires an AVERROR at end of stream; a 0 return is read as a
        // retry by stream protocols and loops.
        let ctx = IoContext::reader(Cursor::new(vec![7u8, 8])).expect("allocation");
        let mut buf = [0u8; 8];
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `buf` is writable for its length; `opaque` is this context's state.
        let first = unsafe { read_packet(opaque, buf.as_mut_ptr(), 8) };
        assert_eq!(first, 2, "the whole source is two bytes");
        assert_eq!(&buf[..2], &[7, 8]);
        // SAFETY: as above; the source is now exhausted.
        let second = unsafe { read_packet(opaque, buf.as_mut_ptr(), 8) };
        assert_eq!(
            second,
            crate::error_codes::EOF,
            "a drained source must report AVERROR_EOF, not 0"
        );
    }

    /// A source that reports more bytes than it was handed, without any `unsafe`
    /// of its own -- which `Read` permits, since it is a safe trait.
    struct OverreportingSource;

    impl std::io::Read for OverreportingSource {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            Ok(buf.len() + 4096)
        }
    }

    impl std::io::Seek for OverreportingSource {
        fn seek(&mut self, _pos: std::io::SeekFrom) -> std::io::Result<u64> {
            Ok(0)
        }
    }

    /// A source that reports `Interrupted` once and then reads normally.
    struct InterruptOnceSource {
        interrupted: bool,
        inner: Cursor<Vec<u8>>,
    }

    impl std::io::Read for InterruptOnceSource {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
            }
            self.inner.read(buf)
        }
    }

    impl std::io::Seek for InterruptOnceSource {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn read_packet_should_reject_a_source_that_reports_more_than_it_was_given() {
        // `Read` is a safe trait, so an implementation can report a length it never
        // wrote without writing any `unsafe`. Forwarding that would move FFmpeg's
        // `buf_end` past the allocation (`fill_buffer` does `buf_end = dst + len`),
        // turning safe caller code into an out-of-bounds read.
        let ctx = IoContext::reader(OverreportingSource).expect("allocation");
        let mut buf = [0u8; 8];
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `buf` is writable for its length; `opaque` is this context's state.
        let ret = unsafe { read_packet(opaque, buf.as_mut_ptr(), 8) };
        assert_eq!(
            ret,
            crate::error_codes::EIO,
            "an over-reported length must be refused, not passed to FFmpeg"
        );
    }

    #[test]
    fn read_packet_should_retry_an_interrupted_read_rather_than_report_zero() {
        // `avio.h`: this callback "must never return 0 but rather a proper AVERROR
        // code", and `read_packet_wrapper` asserts it. Returning 0 for a retryable
        // interrupt reads as a short read, i.e. a silently truncated demux.
        let source = InterruptOnceSource {
            interrupted: false,
            inner: Cursor::new(vec![5u8, 6, 7]),
        };
        let ctx = IoContext::reader(source).expect("allocation");
        let mut buf = [0u8; 8];
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `buf` is writable for its length; `opaque` is this context's state.
        let ret = unsafe { read_packet(opaque, buf.as_mut_ptr(), 8) };
        assert_eq!(ret, 3, "the interrupt must be retried, not reported");
        assert_eq!(&buf[..3], &[5, 6, 7]);
    }

    #[test]
    fn seek_should_reject_a_negative_absolute_position() {
        // Answering an impossible seek with position 0 would be a different, valid
        // seek -- a silently wrong answer where an error is available.
        let ctx = IoContext::reader(Cursor::new(vec![0u8; 16])).expect("allocation");
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `opaque` is this context's state.
        let ret = unsafe { seek(opaque, -8, 0) };
        assert_eq!(ret, i64::from(crate::error_codes::EINVAL));
    }

    #[test]
    fn a_panicking_source_should_return_an_error_instead_of_unwinding() {
        let ctx = IoContext::reader(PanickingSource).expect("allocation");
        let mut buf = [0u8; 4];
        // SAFETY: the context is alive; `buf` is writable for its length.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: as above. The panic inside must be caught by `with_state`; if it
        //         were not, this test would abort the process rather than fail.
        let ret = unsafe { read_packet(opaque, buf.as_mut_ptr(), 4) };
        assert_eq!(
            ret,
            crate::error_codes::EIO,
            "a panicking source must surface as EIO"
        );
    }

    #[test]
    fn seek_should_answer_avseek_size_without_moving_the_position() {
        let ctx = IoContext::reader(Cursor::new(vec![0u8; 40])).expect("allocation");
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `opaque` is this context's state.
        let moved = unsafe { seek(opaque, 10, 0) };
        assert_eq!(moved, 10);
        // SAFETY: as above.
        let size = unsafe { seek(opaque, 0, AVSEEK_SIZE) };
        assert_eq!(size, 40, "AVSEEK_SIZE must report the stream length");
        // SAFETY: as above.
        let still_there = unsafe { seek(opaque, 0, 1) };
        assert_eq!(
            still_there, 10,
            "answering AVSEEK_SIZE must leave the position untouched"
        );
    }

    #[test]
    fn seek_should_mask_avseek_force_off_the_whence() {
        // `AVSEEK_FORCE` is a hint OR-ed onto the whence. Reading it as part of the
        // position would make every forced seek an EINVAL.
        let ctx = IoContext::reader(Cursor::new(vec![0u8; 16])).expect("allocation");
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `opaque` is this context's state.
        let forced = unsafe { seek(opaque, 4, AVSEEK_FORCE) };
        assert_eq!(forced, 4, "SEEK_SET | AVSEEK_FORCE must still seek");
    }

    #[test]
    fn write_packet_should_forward_bytes_to_the_sink() {
        let ctx = IoContext::writer(Cursor::new(Vec::new())).expect("allocation");
        let data = [1u8, 2, 3];
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `data` is readable for its length; `opaque` is this context's state.
        let written = unsafe { write_packet(opaque, data.as_ptr(), 3) };
        assert_eq!(written, 3);
    }

    #[test]
    fn a_reader_context_should_reject_a_write() {
        // The two states are distinct, so a mis-wired context reports an error
        // rather than writing into a source.
        let ctx = IoContext::reader(Cursor::new(vec![0u8; 4])).expect("allocation");
        let data = [1u8];
        // SAFETY: the context is alive and owns the state `opaque` points at.
        let opaque = unsafe { (*ctx.as_ptr()).opaque };
        // SAFETY: `data` is readable for its length; `opaque` is this context's state.
        let ret = unsafe { write_packet(opaque, data.as_ptr(), 1) };
        assert_eq!(ret, crate::error_codes::EIO);
    }
}
