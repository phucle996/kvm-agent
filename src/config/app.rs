use std::time::Duration;

#[derive(Clone, Debug)]
pub enum AppEnvironment {
    Dev,
    Prod,
}

impl AppEnvironment {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "dev" | "development" => Ok(Self::Dev),
            "prod" | "production" => Ok(Self::Prod),
            _ => Err(format!(
                "invalid APP_ENV value '{raw}', expected dev|development|prod|production"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prod => "prod",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppSection {
    pub name: String,
    pub environment: AppEnvironment,
    pub node_id: String,
    pub zone_id: String,
    pub shutdown_timeout: Duration,
}

impl AppSection {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("APP_NAME must not be empty".to_string());
        }
        if self.node_id.trim().is_empty() {
            return Err("APP_NODE_ID must not be empty".to_string());
        }
        if self.zone_id.trim().is_empty() {
            return Err("APP_ZONE_ID must not be empty".to_string());
        }
        if self.shutdown_timeout.is_zero() {
            return Err("SHUTDOWN_TIMEOUT_SEC must be > 0".to_string());
        }
        Ok(())
    }
}
