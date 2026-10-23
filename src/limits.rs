use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Hash)]
pub struct Limits {
    pub memory_mib: usize,
    pub timeout_seconds: u64,
    pub query_steps: usize,
    pub workers: usize,
    pub scalar_steps: usize,
    pub alternatives: usize,
    pub call_depth: usize,
    pub function_scratch_mib: usize,
    pub query_scratch_mib: usize,
}

static CONFIG: OnceLock<Result<Limits, String>> = OnceLock::new();

pub fn configuration() -> Result<Limits, String> {
    CONFIG.get_or_init(|| {
        let mut values = [10240usize, 7200, 100000, 4, 1000, 32, 256, 16, 256];
        for (index, name) in ["IRQ_CHECK_MEMORY_MIB", "IRQ_CHECK_TIMEOUT_SECONDS", "IRQ_CHECK_QUERY_STEPS", "IRQ_CHECK_WORKERS", "IRQ_CHECK_SCALAR_STEPS", "IRQ_CHECK_ALTERNATIVES", "IRQ_CHECK_CALL_DEPTH", "IRQ_CHECK_FUNCTION_SCRATCH_MIB", "IRQ_CHECK_QUERY_SCRATCH_MIB"].iter().enumerate() {
            match std::env::var(name) {
                Ok(value) => {
                    values[index] = value.parse().ok().filter(|value| *value > 0)
                        .ok_or_else(|| format!("{name} must be a positive integer"))?;
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(error) => return Err(format!("cannot read {name}: {error}")),
            }
        }
        if values[0] > 10240 {
            return Err("IRQ_CHECK_MEMORY_MIB must not exceed 10240".into());
        }
        if values[7] > values[0] || values[8] > values[0] {
            return Err("local scratch limits must not exceed IRQ_CHECK_MEMORY_MIB".into());
        }
        if values[2].checked_mul(10).is_none() {
            return Err("IRQ_CHECK_QUERY_STEPS is too large".into());
        }
        let available = std::thread::available_parallelism().map(usize::from).unwrap_or(1);
        Ok(Limits { memory_mib: values[0], timeout_seconds: values[1] as u64, query_steps: values[2], workers: values[3].min(available), scalar_steps: values[4], alternatives: values[5], call_depth: values[6], function_scratch_mib: values[7], query_scratch_mib: values[8] })
    }).clone()
}
