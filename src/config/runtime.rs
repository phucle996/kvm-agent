#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub driver: String,
    pub redis_url: String,
}

impl RuntimeConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.driver.trim().is_empty() {
            return Err("RUNTIME_DRIVER must not be empty".to_string());
        }
        if self.redis_url.trim().is_empty() {
            return Err("REDIS_URL must not be empty".to_string());
        }
        Ok(())
    }
}
