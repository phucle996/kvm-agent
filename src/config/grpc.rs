#[derive(Clone, Debug)]
pub struct GrpcConfig {
    pub bind_addr: String,
}

impl GrpcConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.bind_addr.trim().is_empty() {
            return Err("GRPC_BIND_ADDR must not be empty".to_string());
        }
        Ok(())
    }
}
