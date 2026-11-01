#[path = "../../../src/limits.rs"]
mod configuration;
pub use configuration::{Limits, configuration};

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

static STARTED: OnceLock<Instant> = OnceLock::new();
static STOP: AtomicU8 = AtomicU8::new(0);
static MONITOR: OnceLock<()> = OnceLock::new();

pub fn begin_analysis() {
    STARTED.get_or_init(Instant::now);
}

pub fn checkpoint() -> bool {
    if STOP.load(Ordering::Relaxed) != 0 {
        return false;
    }
    if let Some(started) = STARTED.get() {
        if let Ok(limits) = configuration() {
            if started.elapsed() >= Duration::from_secs(limits.timeout_seconds) {
                STOP.store(1, Ordering::Relaxed);
                return false;
            }
        }
    }
    true
}

pub fn stop_reason() -> Option<&'static str> {
    match STOP.load(Ordering::Relaxed) {
        0 => None,
        1 => Some("the analysis time limit was reached; no partial result is accepted"),
        2 => Some("the analysis memory limit was reached; no partial result is accepted"),
        _ => Some("memory accounting failed; no partial result is accepted"),
    }
}

pub fn install_process_limit() -> Result<(), String> {
    let limits = configuration()?;
    if MONITOR.get().is_some() {
        return Ok(());
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    return Err("this build cannot enforce the analysis memory limit on this host".into());
    #[cfg(target_os = "linux")]
    {
        let requested = libc::rlim_t::try_from(limits.memory_mib)
            .ok()
            .and_then(|value| value.checked_mul(1024 * 1024))
            .ok_or_else(|| "the driver memory limit is too large for this host".to_owned())?;
        let mut cap = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut cap) } != 0 {
            return Err(format!("cannot read the driver memory limit: {}", std::io::Error::last_os_error()));
        }
        let effective = cap.rlim_cur.min(cap.rlim_max).min(requested);
        let soft_bytes = (effective as u64).saturating_mul(7) / 8;
        if linux_virtual_bytes()? >= soft_bytes {
            return Err("the driver already exceeds the analysis memory allowance".into());
        }
        cap.rlim_cur = effective;
        cap.rlim_max = effective;
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &cap) } != 0 {
            return Err(format!("cannot enforce the driver memory limit: {}", std::io::Error::last_os_error()));
        }
        STARTED.get_or_init(Instant::now);
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("irq-check-limits".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let mut ready = Some(ready);
                let mut stopped_at = None;
                loop {
                    let reason = match linux_virtual_bytes() {
                        Ok(bytes) if bytes >= soft_bytes => 2,
                        Ok(_) => 0,
                        Err(_) => 3,
                    };
                    if reason != 0 {
                        let _ = STOP.compare_exchange(0, reason, Ordering::Relaxed, Ordering::Relaxed);
                    }
                    let running = checkpoint();
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(running);
                    }
                    if !running {
                        let stopped = stopped_at.get_or_insert_with(|| {
                            eprintln!("error: interrupt call analysis did not reach a complete result\nnote: {}",
                                stop_reason().unwrap_or("analysis stopped"));
                            Instant::now()
                        });
                        if stopped.elapsed() >= Duration::from_secs(2) {
                            std::process::exit(2);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|error| format!("cannot start the driver limit monitor: {error}"))?;
        if !started.recv().map_err(|error| format!("cannot start the driver limit monitor: {error}"))? {
            return Err(stop_reason().unwrap_or("analysis stopped").into());
        }
        let _ = MONITOR.set(());
        Ok(())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            JobObjectExtendedLimitInformation,
        };
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
        use windows_sys::Win32::System::Threading::{CreateTimerQueueTimer, GetCurrentProcess, WT_EXECUTEDEFAULT};

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(format!("cannot create the driver memory limit: {}", unsafe { GetLastError() }));
        }
        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        information.ProcessMemoryLimit = limits.memory_mib * 1024 * 1024;
        let set = unsafe {
            SetInformationJobObject(job, JobObjectExtendedLimitInformation,
                &information as *const _ as *const _, std::mem::size_of_val(&information) as u32)
        };
        if set == 0 || unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) } == 0 {
            let error = unsafe { GetLastError() };
            unsafe { CloseHandle(job) };
            return Err(format!("cannot enforce the driver memory limit: {error}"));
        }
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of_val(&counters) as u32;
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters as *mut _ as *mut _, counters.cb) } == 0 {
            return Err(format!("cannot read driver memory: {}", unsafe { GetLastError() }));
        }
        let soft_bytes = information.ProcessMemoryLimit.saturating_mul(7) / 8;
        STARTED.get_or_init(Instant::now);
        let mut timer = std::ptr::null_mut();
        if unsafe {
            CreateTimerQueueTimer(&mut timer, std::ptr::null_mut(), Some(sample_process_limit),
                soft_bytes as *const core::ffi::c_void, 0, 100, WT_EXECUTEDEFAULT)
        } == 0 {
            return Err(format!("cannot start the driver limit monitor: Windows error {}", unsafe { GetLastError() }));
        }
        let _ = MONITOR.set(());
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn linux_virtual_bytes() -> Result<u64, String> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("cannot read driver memory: {error}"))?;
    let value = status.lines()
        .find_map(|line| line.strip_prefix("VmSize:"))
        .ok_or_else(|| "driver virtual memory accounting is unavailable".to_owned())?;
    let mut fields = value.split_whitespace();
    let bytes = fields.next()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|value| value.checked_mul(1024));
    if fields.next() != Some("kB") || fields.next().is_some() {
        return Err("invalid driver virtual memory accounting".into());
    }
    bytes.ok_or_else(|| "invalid driver virtual memory accounting".into())
}

#[cfg(windows)]
unsafe extern "system" fn sample_process_limit(parameter: *mut core::ffi::c_void, _: bool) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    static STOPPED_AT: OnceLock<Instant> = OnceLock::new();
    let mut memory: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    memory.cb = std::mem::size_of_val(&memory) as u32;
    let reason = if unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut memory as *mut _ as *mut _, memory.cb)
    } == 0 {
        3
    } else if memory.PrivateUsage >= parameter as usize {
        2
    } else {
        0
    };
    if reason != 0 {
        let _ = STOP.compare_exchange(0, reason, Ordering::Relaxed, Ordering::Relaxed);
    }
    if !checkpoint() {
        let stopped = STOPPED_AT.get_or_init(|| {
            eprintln!("error: interrupt call analysis did not reach a complete result\nnote: {}",
                stop_reason().unwrap_or("analysis stopped"));
            Instant::now()
        });
        if stopped.elapsed() >= Duration::from_secs(2) {
            std::process::exit(2);
        }
    }
}
