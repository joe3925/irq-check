use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Reporting {
    pub stats: bool,
    pub directory: Option<PathBuf>,
    pub explain: bool,
}

static CONFIG: OnceLock<Reporting> = OnceLock::new();

pub fn configuration() -> &'static Reporting {
    CONFIG.get_or_init(|| Reporting {
        stats: std::env::var_os("IRQ_CHECK_STATS").is_some(),
        directory: std::env::var_os("IRQ_CHECK_DUMP_DIR").map(PathBuf::from),
        explain: std::env::var_os("IRQ_CHECK_EXPLAIN").is_some(),
    })
}
