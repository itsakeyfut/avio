//! `ensure_initialized` must install the `av_log` bridge (#1599).
//!
//! This lives in its own integration binary on purpose. The unit tests in
//! `log_bridge` call `install_log_bridge` directly, and once its `Once` has
//! fired a later call proves nothing about *who* fired it — so a version of
//! `ensure_initialized` that forgot to install anything would still pass them.
//! In a fresh process `ensure_initialized` is the only thing that installs, so a
//! missing wire shows up here as an empty record list.
//!
//! That wire is how every other crate in the family gets the bridge: 22 call
//! sites reach `ensure_initialized`, and none of them mention `log_bridge`.

use std::os::raw::c_int;
use std::sync::Mutex;

static RECORDS: Mutex<Vec<(log::Level, String)>> = Mutex::new(Vec::new());
static COLLECTOR: Collector = Collector;

struct Collector;

impl log::Log for Collector {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if record.target() == "ffmpeg" {
            RECORDS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((record.level(), record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

#[test]
fn ensure_initialized_should_install_the_log_bridge() {
    log::set_logger(&COLLECTOR).expect("the first logger set in this test binary");
    log::set_max_level(log::LevelFilter::Trace);

    // The only install in this process.
    ff_sys::ensure_initialized();
    ff_sys::set_log_level(log::LevelFilter::Trace);

    // SAFETY: `av_log` accepts a null `avcl` (no AVClass context); the format
    // string is null-terminated and its one specifier matches the argument that
    // follows.
    unsafe {
        ff_sys::av_log(
            std::ptr::null_mut(),
            ff_sys::AV_LOG_WARNING as c_int,
            c"ff-sys ensure_initialized wiring probe %d\n".as_ptr(),
            1599_i32,
        );
    }

    let records = RECORDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (level, message) = records
        .first()
        .expect("ensure_initialized must route FFmpeg's messages to the log facade");
    assert_eq!(
        *level,
        log::Level::Warn,
        "AV_LOG_WARNING must arrive as Warn"
    );
    assert!(
        message.contains("wiring probe"),
        "the probe message must arrive; got {message:?}"
    );
    assert!(
        message.contains("1599"),
        "the formatted argument must be substituted; got {message:?}"
    );
}
