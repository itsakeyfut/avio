//! Bridge FFmpeg's `av_log` output into the Rust `log` facade.
//!
//! FFmpeg writes its internal diagnostics to stderr by default, bypassing
//! whatever logging the consuming application configured. [`install_log_bridge`]
//! replaces that with a callback forwarding each message to the `log` crate
//! under the `ffmpeg` target, so a backend filter (`RUST_LOG=ffmpeg=warn` and
//! the like) governs FFmpeg's output alongside the rest of the workspace's.
//!
//! [`ensure_initialized`](crate::ensure_initialized) installs the bridge, so
//! every crate in the family gets it without doing anything.
//!
//! # Formatting without interpreting `va_list`
//!
//! FFmpeg hands the callback a `va_list`, which Rust cannot read on stable
//! (`c_variadic` is unstable). It does not need to: the list is forwarded
//! **once, untouched**, to `av_log_format_line2`, and FFmpeg does the
//! formatting.
//!
//! The callback's parameter type cannot simply be spelled
//! [`va_list`](crate::va_list), because that alias does not always match what
//! bindgen puts in *parameter* position:
//!
//! | target | `va_list` typedef | parameter type |
//! |---|---|---|
//! | x86_64 Linux / Intel macOS | `[__va_list_tag; 1]` | `*mut __va_list_tag` |
//! | aarch64 Linux, aarch64 macOS, Windows MSVC | pointer or struct | the alias |
//!
//! Where the typedef is an array, C's array-to-pointer adjustment strips it off
//! parameters, and bindgen follows clang in dropping the typedef sugar there.
//! Naming the alias then fails to compile on exactly the platforms that build
//! against real FFmpeg in CI, while passing on Windows where the two coincide.
//! [`VaListArg`] selects the right shape from a `build.rs` cfg derived from the
//! generated bindings.
//!
//! # The level check comes with the callback
//!
//! `av_vlog` dispatches to the installed callback unconditionally; the
//! `av_log_set_level` threshold is applied inside `av_log_default_callback`, not
//! before it. Replacing that callback therefore takes the check over, which is
//! why [`log_callback`] consults `av_log_get_level` itself. Without it
//! [`set_log_level`] would still update FFmpeg's global and still round-trip
//! through [`log_level`], while silently filtering nothing.
//!
//! # Deviations from `av_log_default_callback`
//!
//! - **Repeated lines are not collapsed.** FFmpeg's own callback folds an
//!   identical line into "Last message repeated N times"; doing that here would
//!   mean shared mutable state in a callback FFmpeg invokes from its internal
//!   threads, so a component that repeats a warning produces one record per
//!   repeat. A `log` backend can deduplicate if it matters.
//! - **A level's colour tint is discarded.** `AV_LOG_C` tint bits are masked off
//!   the level before it is mapped, since `log` has no colour channel.
//! - **The message prefix is per-record.** FFmpeg carries `print_prefix` across
//!   calls so a line emitted as several fragments is prefixed once. Each record
//!   here identifies itself instead, so such a line arrives as several records.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Once;

use log::LevelFilter;

/// The type bindgen gave the `va_list` parameter of `av_log_format_line2` and of
/// the `av_log_set_callback` function pointer on this target.
///
/// `build.rs` sets `cfg(va_list_tag)` when the generated bindings mention
/// `__va_list_tag`, which is exactly the case where `va_list` is an array
/// typedef and parameters therefore decay to a pointer. See the module docs.
#[cfg(va_list_tag)]
type VaListArg = *mut crate::__va_list_tag;
/// The type bindgen gave the `va_list` parameter on this target (the alias
/// itself, where no array-to-pointer adjustment happened).
#[cfg(not(va_list_tag))]
type VaListArg = crate::va_list;

/// Installation guard: FFmpeg keeps one global callback pointer, so the bridge
/// is installed exactly once no matter how many threads race to do it.
static INSTALL: Once = Once::new();

/// The `log` target every bridged message carries, so consumers can filter
/// FFmpeg's chatter separately from the workspace's own records.
const TARGET: &str = "ffmpeg";

/// Buffer for one formatted message. Matches the size FFmpeg's own default
/// callback uses; a longer line is truncated rather than allocated for, which is
/// what keeps the callback allocation-free.
const LINE_CAPACITY: usize = 1024;

// The bindgen constants, cast once. `AV_LOG_QUIET` comes through as `i32` and
// the rest as `u32`, so comparing them needs a common type.
const QUIET: c_int = crate::AV_LOG_QUIET as c_int;
const ERROR: c_int = crate::AV_LOG_ERROR as c_int;
const WARNING: c_int = crate::AV_LOG_WARNING as c_int;
const INFO: c_int = crate::AV_LOG_INFO as c_int;
const VERBOSE: c_int = crate::AV_LOG_VERBOSE as c_int;
const TRACE: c_int = crate::AV_LOG_TRACE as c_int;

/// Map an `AV_LOG_*` level onto the `log` filter that would record it.
///
/// Ranges rather than equality: `av_log` takes an arbitrary `int` and FFmpeg
/// compares it numerically, so a level between two named constants has to land
/// on the next one that would print it. Mapping `AV_LOG_QUIET` to `Off` also
/// gives the callback its "drop this" case for free, since
/// [`LevelFilter::to_level`] is `None` there.
///
/// `AV_LOG_DEBUG` maps to `Trace` rather than `Debug` because FFmpeg's debug
/// level is per-frame chatter, which belongs at Rust's most verbose level.
fn av_to_filter(av_level: c_int) -> LevelFilter {
    if av_level <= QUIET {
        LevelFilter::Off
    } else if av_level <= ERROR {
        LevelFilter::Error
    } else if av_level <= WARNING {
        LevelFilter::Warn
    } else if av_level <= INFO {
        LevelFilter::Info
    } else if av_level <= VERBOSE {
        LevelFilter::Debug
    } else {
        LevelFilter::Trace
    }
}

/// The inverse of [`av_to_filter`]: the `AV_LOG_*` threshold that lets exactly
/// the messages `filter` admits through.
fn filter_to_av(filter: LevelFilter) -> c_int {
    match filter {
        LevelFilter::Off => QUIET,
        LevelFilter::Error => ERROR,
        LevelFilter::Warn => WARNING,
        LevelFilter::Info => INFO,
        LevelFilter::Debug => VERBOSE,
        LevelFilter::Trace => TRACE,
    }
}

/// Forward one FFmpeg message to the `log` facade.
///
/// # Safety
///
/// Installed only through [`install_log_bridge`], so FFmpeg is the sole caller
/// and supplies the arguments under `av_log`'s contract: `fmt` is a valid
/// null-terminated format string and `vl` the matching argument list. `vl` is
/// forwarded to `av_log_format_line2` exactly once and never inspected here,
/// which is what makes this sound without `c_variadic`. The function holds no
/// state, so FFmpeg may call it from any thread.
unsafe extern "C" fn log_callback(
    avcl: *mut c_void,
    level: c_int,
    fmt: *const c_char,
    vl: VaListArg,
) {
    // FFmpeg is a C caller, so an unwind escaping this function aborts the whole
    // process. The body ends in a call into whatever `log::Log` the application
    // installed - arbitrary code this crate does not control, and a panicking
    // logger (a poisoned mutex, a closed pipe) is an ordinary bug rather than an
    // exotic one. Swallow the payload: there is nowhere to report it, and
    // re-entering `log` to complain risks a second panic.
    //
    // SAFETY: forwards its arguments unchanged to `log_callback_impl`, whose
    // contract is the same one FFmpeg guarantees for this callback.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        log_callback_impl(avcl, level, fmt, vl);
    }));
}

/// The body of [`log_callback`], separated so the `extern "C"` boundary above is
/// nothing but the unwind guard.
///
/// # Safety
///
/// Same contract as [`log_callback`]: `fmt` is a valid null-terminated format
/// string and `vl` its matching argument list, and `vl` is consumed exactly once.
unsafe fn log_callback_impl(avcl: *mut c_void, level: c_int, fmt: *const c_char, vl: VaListArg) {
    // A level can carry a colour tint in its high bits (`AV_LOG_C(x) = (x) << 8`,
    // libavutil/log.h), which `av_log_default_callback` masks off before
    // thresholding. Without this a tinted warning arrives as ~34328, fails every
    // threshold, and is silently dropped. The tint itself is discarded: the `log`
    // facade has no colour channel.
    let level = if level >= 0 { level & 0xff } else { level };

    // FFmpeg does *not* apply `av_log_set_level` before dispatching: `av_vlog`
    // calls the installed callback unconditionally, and the threshold check
    // lives inside `av_log_default_callback`. Replacing that callback therefore
    // takes the check over, and dropping this line silently turns
    // `set_log_level` into a no-op.
    //
    // SAFETY: `av_log_get_level` only reads an FFmpeg global.
    if level > unsafe { crate::av_log_get_level() } {
        return;
    }
    let Some(mapped) = av_to_filter(level).to_level() else {
        return;
    };
    // Then the facade's own threshold, so a message no logger would record does
    // not cost a `vsnprintf`. `max_level` is a plain atomic load, and this runs
    // on FFmpeg's decode threads. (`log::log!` filters again, so unlike the
    // check above this one changes cost, not behaviour.)
    if mapped > log::max_level() {
        return;
    }
    if fmt.is_null() {
        return;
    }

    let mut line = [0 as c_char; LINE_CAPACITY];
    // FFmpeg carries this across calls so a line emitted as several fragments is
    // prefixed once; keeping that state would mean sharing it across FFmpeg's
    // threads, so every record identifies itself instead.
    let mut print_prefix: c_int = 1;

    // SAFETY: `line` is a live buffer of `LINE_CAPACITY` elements that outlives
    // the call, and `LINE_CAPACITY` is what we declare its size to be. `fmt` is
    // non-null (checked above) and `vl` matches it, both guaranteed by
    // `av_log`'s contract with its callback. `av_log_format_line2` consumes `vl`
    // exactly once and null-terminates within `line_size`.
    let written = unsafe {
        crate::av_log_format_line2(
            avcl,
            level,
            fmt,
            vl,
            line.as_mut_ptr(),
            LINE_CAPACITY as c_int,
            &raw mut print_prefix,
        )
    };
    if written <= 0 {
        return;
    }

    // SAFETY: `av_log_format_line2` reported a positive length, so it wrote a
    // null-terminated string into `line`.
    let message = unsafe { CStr::from_ptr(line.as_ptr()) }.to_string_lossy();
    // FFmpeg terminates its lines; `log` records are lines already.
    log::log!(target: TARGET, mapped, "{}", message.trim_end());
}

/// Route FFmpeg's internal diagnostics into the `log` facade under the `ffmpeg`
/// target, instead of letting them go to stderr.
///
/// Idempotent and safe to call from any thread: FFmpeg holds a single global
/// callback pointer, and repeat calls do nothing.
/// [`ensure_initialized`](crate::ensure_initialized) already calls this, so most
/// callers never need to.
///
/// This changes **process-global** FFmpeg state. An application that wants
/// FFmpeg's messages on stderr should not call `ensure_initialized` (or should
/// install its own callback afterwards).
pub fn install_log_bridge() {
    INSTALL.call_once(|| {
        // SAFETY: `av_log_set_callback` only stores the pointer in an FFmpeg
        // global. `log_callback` is a `'static` function whose signature is the
        // one FFmpeg declares for the callback (see [`VaListArg`]).
        unsafe { crate::av_log_set_callback(Some(log_callback)) };
    });
}

/// Set the threshold below which FFmpeg's messages are discarded.
///
/// This writes FFmpeg's own global level (`av_log_set_level`), which the bridge
/// then enforces: FFmpeg dispatches to an installed callback *without* checking
/// the level, so the check belongs to whoever replaced the default callback.
/// A message dropped by it is never formatted.
///
/// The `log` backend filters again, so the effective level is the stricter of
/// the two. FFmpeg's default is [`LevelFilter::Info`].
pub fn set_log_level(level: LevelFilter) {
    // SAFETY: `av_log_set_level` only stores an int in an FFmpeg global.
    unsafe { crate::av_log_set_level(filter_to_av(level)) };
}

/// The level FFmpeg is currently logging at, as the `log` filter that matches
/// it.
///
/// Round-trips with [`set_log_level`]. Reading back a level FFmpeg was given by
/// other means can only be approximate, since several `AV_LOG_*` values map onto
/// one [`LevelFilter`].
#[must_use]
pub fn log_level() -> LevelFilter {
    // SAFETY: `av_log_get_level` only reads an FFmpeg global.
    let level = unsafe { crate::av_log_get_level() };
    av_to_filter(level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialises the tests that touch FFmpeg's global log level or the process
    /// logger, which cargo would otherwise run concurrently.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Records the bridge produced, so a test can assert on them.
    static RECORDS: Mutex<Vec<(log::Level, String)>> = Mutex::new(Vec::new());
    /// A message the collector deliberately panics on, so a test can prove the
    /// callback's unwind guard holds. A real backend panics for duller reasons
    /// (a poisoned mutex, a closed pipe); the effect at the FFI boundary is the
    /// same.
    const PANIC_PROBE: &str = "ff-sys log bridge panic probe";
    static COLLECTOR: Collector = Collector;
    static LOGGER_INIT: Once = Once::new();

    struct Collector;

    impl log::Log for Collector {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            if record.target() == TARGET {
                let message = record.args().to_string();
                assert!(
                    !message.contains(PANIC_PROBE),
                    "deliberate panic from the test log backend"
                );
                RECORDS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((record.level(), message));
            }
        }

        fn flush(&self) {}
    }

    /// Run `f` with the collecting logger installed, the bridge live, and the
    /// record list empty. Restores FFmpeg's log level afterwards so the tests
    /// stay independent of each other.
    fn with_collector<T>(f: impl FnOnce() -> T) -> T {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        LOGGER_INIT.call_once(|| {
            log::set_logger(&COLLECTOR).expect("no other logger in this test binary");
        });
        install_log_bridge();
        let previous_ffmpeg = log_level();
        log::set_max_level(LevelFilter::Trace);
        RECORDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        let out = f();

        set_log_level(previous_ffmpeg);
        log::set_max_level(LevelFilter::Trace);
        out
    }

    /// The records collected so far whose message contains `marker`.
    ///
    /// Scoped by marker rather than taken wholesale because `log::set_logger` is
    /// process-global: the ~146 other tests in this binary build real FFmpeg
    /// contexts, and once `set_log_level` opens the threshold their messages land
    /// in `RECORDS` too (measured: 95 foreign records in a 2.5s window, from
    /// swresample and swscale). Asserting on `first()` or on a total count would
    /// be a race against whichever test happens to run alongside.
    fn records_matching(marker: &str) -> Vec<(log::Level, String)> {
        RECORDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, message)| message.contains(marker))
            .cloned()
            .collect()
    }

    #[test]
    fn av_to_filter_should_map_each_ffmpeg_level() {
        let cases = [
            (crate::AV_LOG_QUIET as c_int, LevelFilter::Off),
            (crate::AV_LOG_PANIC as c_int, LevelFilter::Error),
            (crate::AV_LOG_FATAL as c_int, LevelFilter::Error),
            (crate::AV_LOG_ERROR as c_int, LevelFilter::Error),
            (crate::AV_LOG_WARNING as c_int, LevelFilter::Warn),
            (crate::AV_LOG_INFO as c_int, LevelFilter::Info),
            (crate::AV_LOG_VERBOSE as c_int, LevelFilter::Debug),
            (crate::AV_LOG_DEBUG as c_int, LevelFilter::Trace),
            (crate::AV_LOG_TRACE as c_int, LevelFilter::Trace),
        ];
        for (av_level, expected) in cases {
            assert_eq!(
                av_to_filter(av_level),
                expected,
                "AV_LOG level {av_level} must map to {expected}"
            );
        }
    }

    #[test]
    fn av_to_filter_should_map_values_between_named_levels() {
        // `av_log` takes an arbitrary int, so the mapping has to be a range. A
        // lookup table keyed on the named constants would fall through on all of
        // these.
        assert_eq!(
            av_to_filter(4),
            LevelFilter::Error,
            "between PANIC and FATAL"
        );
        assert_eq!(
            av_to_filter(20),
            LevelFilter::Warn,
            "between ERROR and WARNING"
        );
        assert_eq!(
            av_to_filter(28),
            LevelFilter::Info,
            "between WARNING and INFO"
        );
        assert_eq!(av_to_filter(60), LevelFilter::Trace, "above TRACE");
        assert_eq!(av_to_filter(-100), LevelFilter::Off, "below QUIET");
    }

    #[test]
    fn filter_to_av_should_round_trip_through_av_to_filter() {
        for filter in [
            LevelFilter::Off,
            LevelFilter::Error,
            LevelFilter::Warn,
            LevelFilter::Info,
            LevelFilter::Debug,
            LevelFilter::Trace,
        ] {
            assert_eq!(
                av_to_filter(filter_to_av(filter)),
                filter,
                "{filter} must survive the round trip through AV_LOG levels"
            );
        }
    }

    #[test]
    fn install_log_bridge_should_be_idempotent_across_threads() {
        // FFmpeg holds one global callback pointer, so racing installs must
        // collapse onto a single one rather than tearing.
        let threads: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(install_log_bridge))
            .collect();
        for thread in threads {
            thread.join().expect("install_log_bridge must not panic");
        }
        install_log_bridge();
    }

    #[test]
    fn set_log_level_should_round_trip_through_ffmpeg() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = log_level();
        for filter in [
            LevelFilter::Off,
            LevelFilter::Error,
            LevelFilter::Warn,
            LevelFilter::Info,
            LevelFilter::Debug,
            LevelFilter::Trace,
        ] {
            set_log_level(filter);
            assert_eq!(
                log_level(),
                filter,
                "{filter} must survive a round trip through FFmpeg's global level"
            );
        }
        set_log_level(previous);
    }

    #[test]
    fn av_log_should_reach_the_log_facade_with_formatted_arguments() {
        with_collector(|| {
            set_log_level(LevelFilter::Trace);

            // The format specifiers are the point. A bridge that ignored `vl`
            // and copied the format string verbatim would still produce a
            // record, so only asserting that the *substituted* values appear
            // proves the va_list reached `av_log_format_line2`.
            //
            // SAFETY: `av_log` accepts a null `avcl` (no AVClass context); the
            // format string is null-terminated and its specifiers match the
            // arguments that follow.
            unsafe {
                crate::av_log(
                    std::ptr::null_mut(),
                    crate::AV_LOG_ERROR as c_int,
                    c"ff-sys log bridge probe %d %s\n".as_ptr(),
                    1599_i32,
                    c"marker".as_ptr(),
                );
            }

            let collected = records_matching("ff-sys log bridge probe");
            let (level, message) = collected
                .first()
                .expect("an AV_LOG_ERROR message must reach the log facade");
            assert_eq!(*level, log::Level::Error, "AV_LOG_ERROR must map to Error");
            assert!(
                message.contains("1599"),
                "the %d argument must be substituted; got {message:?}"
            );
            assert!(
                message.contains("marker"),
                "the %s argument must be substituted; got {message:?}"
            );
            assert!(
                !message.contains("%d"),
                "the raw format string must not be recorded; got {message:?}"
            );
            assert!(
                !message.ends_with('\n'),
                "FFmpeg's trailing newline must be trimmed; got {message:?}"
            );
        });
    }

    #[test]
    fn log_callback_should_map_a_tinted_level_by_its_low_byte() {
        with_collector(|| {
            set_log_level(LevelFilter::Trace);

            // `AV_LOG_C(134)` is 134 << 8, so this arrives as 34328. Unmasked it
            // exceeds every threshold and is dropped silently, and would map to
            // Trace rather than Warn if it did get through.
            const TINTED_WARNING: c_int = crate::AV_LOG_WARNING as c_int | (134 << 8);

            // SAFETY: null `avcl` is accepted and the format string is
            // null-terminated with no specifiers to satisfy.
            unsafe {
                crate::av_log(
                    std::ptr::null_mut(),
                    TINTED_WARNING,
                    c"ff-sys log bridge tint probe
"
                    .as_ptr(),
                );
            }

            let collected = records_matching("tint probe");
            let (level, _) = collected
                .first()
                .expect("a tinted warning must not be dropped by the threshold");
            assert_eq!(
                *level,
                log::Level::Warn,
                "the tint must be masked off before mapping, leaving AV_LOG_WARNING"
            );
        });
    }

    #[test]
    fn log_callback_should_contain_a_panic_from_the_log_backend() {
        with_collector(|| {
            set_log_level(LevelFilter::Trace);

            // The collector panics on this one. Without the unwind guard the
            // panic would cross the `extern "C"` boundary and abort the whole
            // process, taking the test binary with it — so merely reaching the
            // next statement is most of the assertion. (A panic message on
            // stderr here is expected.)
            // SAFETY: as above.
            unsafe {
                crate::av_log(
                    std::ptr::null_mut(),
                    crate::AV_LOG_ERROR as c_int,
                    c"ff-sys log bridge panic probe
"
                    .as_ptr(),
                );
            }

            // And the bridge must still work afterwards, not be left wedged.
            // SAFETY: as above.
            unsafe {
                crate::av_log(
                    std::ptr::null_mut(),
                    crate::AV_LOG_ERROR as c_int,
                    c"ff-sys log bridge post-panic probe
"
                    .as_ptr(),
                );
            }
            assert_eq!(
                records_matching("post-panic probe").len(),
                1,
                "the bridge must keep working after a backend panic"
            );
        });
    }

    #[test]
    fn set_log_level_should_stop_ffmpeg_from_passing_lower_priority_messages() {
        with_collector(|| {
            // The `log` facade is wide open, so anything dropped here was
            // dropped by FFmpeg's own threshold.
            set_log_level(LevelFilter::Warn);

            // SAFETY: as in the test above.
            unsafe {
                crate::av_log(
                    std::ptr::null_mut(),
                    crate::AV_LOG_INFO as c_int,
                    c"ff-sys log bridge info probe\n".as_ptr(),
                );
            }
            assert!(
                records_matching("info probe").is_empty(),
                "an Info message must not pass a Warn threshold; got {:?}",
                records_matching("info probe")
            );

            // Non-vacuous: a bridge that recorded nothing at all would satisfy
            // the assertion above, so prove the path is live at a level the
            // threshold admits.
            // SAFETY: as above.
            unsafe {
                crate::av_log(
                    std::ptr::null_mut(),
                    crate::AV_LOG_ERROR as c_int,
                    c"ff-sys log bridge error probe\n".as_ptr(),
                );
            }
            assert_eq!(
                records_matching("error probe").len(),
                1,
                "an Error message must still pass a Warn threshold"
            );
        });
    }
}
