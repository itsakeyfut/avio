//! Compile-parallelism cap for `cargo xtask test` and `cargo xtask gate`.
//!
//! Formula: `min(cores, floor(total_ram_gb / 2))`, clamped to at least 1.
//!
//! The divisor is 2 GB per job rather than the measured ~0.5 GB/core because
//! 0.5 is what makes a 32-core/15.9 GB machine saturate; the linker's own
//! resident set pushes the real per-job appetite to roughly 1.5 GB under full
//! load. With a 2 GB divisor the formula is conservative enough to leave
//! meaningful headroom.
//!
//! On a machine with at least 2 GB per core the formula reduces to
//! `min(cores, cores) = cores`, so the cap does not bind there and the run
//! stays at full parallelism. A unit test asserts this property against the
//! derivation without requiring such a machine to be present.
//!
//! The cap is applied only by `cargo xtask test` and `cargo xtask gate`, not
//! via `.cargo/config.toml`. The ratio of memory to cores is a property of the
//! local machine, not of the project: placing a fixed number in the config
//! would impose it on every contributor and on CI runners whose core counts are
//! much lower, where it would be a pointless constraint. A bare `cargo test`
//! typed directly keeps cargo's default behaviour and is unaffected.

/// Returns the `--jobs` value to pass to cargo.
///
/// Queries the system for core count and total RAM and applies the formula.
/// On any platform where the RAM query fails, the fallback is `usize::MAX`,
/// which makes `floor(MAX / 2) >= cores` hold for any plausible core count,
/// so the formula reduces to `cores` and the cap does not bind.
pub fn cap() -> usize {
    let cores = num_cores();
    let ram_gb = total_ram_gb();
    jobs_cap_for(cores, ram_gb)
}

/// Derivation: `min(cores, floor(total_ram_gb / 2))`, at least 1.
///
/// Separated from [`cap`] so that unit tests can exercise the formula against
/// hypothetical inputs without querying the real machine.
pub fn jobs_cap_for(cores: usize, total_ram_gb: usize) -> usize {
    cores.min(total_ram_gb / 2).max(1)
}

fn num_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn total_ram_gb() -> usize {
    try_total_ram_gb().unwrap_or(usize::MAX)
}

#[cfg(target_os = "linux")]
fn try_total_ram_gb() -> Option<usize> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    // Format: "MemTotal:       16645464 kB"
    content.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?;
        let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
        Some((kb / (1024 * 1024)) as usize)
    })
}

#[cfg(target_os = "macos")]
fn try_total_ram_gb() -> Option<usize> {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    let bytes: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some((bytes / (1024 * 1024 * 1024)) as usize)
}

#[cfg(target_os = "windows")]
fn try_total_ram_gb() -> Option<usize> {
    use std::mem;

    // MEMORYSTATUSEX from <sysinfoapi.h>.  The layout is stable across all
    // supported Windows versions and is declared here to avoid pulling in the
    // `windows` or `winapi` crates, which would break xtask's no-dependency
    // invariant.
    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    unsafe extern "system" {
        fn GlobalMemoryStatusEx(lpbuffer: *mut MemoryStatusEx) -> i32;
    }

    // SAFETY: `status` is zero-initialised, `dw_length` is set to the struct
    // size before the call (required by the API contract), and the pointer is
    // valid for the duration of the call.
    let mut status: MemoryStatusEx = unsafe { mem::zeroed() };
    status.dw_length = mem::size_of::<MemoryStatusEx>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        return None;
    }
    Some((status.ull_total_phys / (1024 * 1024 * 1024)) as usize)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn try_total_ram_gb() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_cap_should_not_bind_when_ram_per_core_is_sufficient() {
        // On a machine with at least 2 GB per core, floor(ram_gb / 2) >= cores,
        // so min(cores, floor(ram_gb / 2)) = cores and the cap does not reduce
        // parallelism below what the hardware provides.
        for cores in [1_usize, 4, 8, 16, 32, 64] {
            let ram_gb = cores * 2; // exactly 2 GB per core — the boundary case
            assert_eq!(
                jobs_cap_for(cores, ram_gb),
                cores,
                "cap should equal cores at exactly 2 GB/core (cores={cores})"
            );
            let ram_gb_healthy = cores * 4; // 4 GB per core — well-resourced
            assert_eq!(
                jobs_cap_for(cores, ram_gb_healthy),
                cores,
                "cap should equal cores at 4 GB/core (cores={cores})"
            );
        }
    }

    #[test]
    fn jobs_cap_should_clamp_when_memory_is_scarce() {
        // Reproduces the measured machine: 32 cores, 15 GB usable
        // → min(32, floor(15/2)) = min(32, 7) = 7
        assert_eq!(jobs_cap_for(32, 15), 7);
        // Same machine but with 16 GB measured (floor(16/2) = 8)
        assert_eq!(jobs_cap_for(32, 16), 8);
        // Always at least 1, even with zero RAM reported
        assert_eq!(jobs_cap_for(4, 0), 1);
        assert_eq!(jobs_cap_for(1, 0), 1);
    }
}
