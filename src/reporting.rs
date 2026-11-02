use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Reporting {
    pub stats: bool,
    pub directory: Option<PathBuf>,
    pub explain: bool,
}

static CONFIG: OnceLock<Reporting> = OnceLock::new();

pub fn cpu_seconds() -> Option<f64> {
    if !configuration().stats { return None; }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let mut created = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut exited = created;
        let mut kernel = created;
        let mut user = created;
        if unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
            return None;
        }
        Some((((kernel.dwHighDateTime as u64) << 32 | kernel.dwLowDateTime as u64)
            + ((user.dwHighDateTime as u64) << 32 | user.dwLowDateTime as u64)) as f64 / 10_000_000.0)
    }
    #[cfg(not(windows))]
    None
}

pub fn configuration() -> &'static Reporting {
    CONFIG.get_or_init(|| Reporting {
        stats: std::env::var_os("IRQ_CHECK_STATS").is_some(),
        directory: std::env::var_os("IRQ_CHECK_DUMP_DIR").map(PathBuf::from),
        explain: std::env::var_os("IRQ_CHECK_EXPLAIN").is_some(),
    })
}
