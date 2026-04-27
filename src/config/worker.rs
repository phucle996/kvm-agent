#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub max_workers: usize,
}

impl WorkerConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_workers == 0 {
            return Err("WORKER_MAX must be > 0".to_string());
        }
        Ok(())
    }
}
