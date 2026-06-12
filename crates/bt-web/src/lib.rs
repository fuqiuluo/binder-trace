#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WebConfig {
    pub bind_address: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:0".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WebApp {
    config: WebConfig,
}

impl WebApp {
    pub const fn new(config: WebConfig) -> Self {
        Self { config }
    }

    pub const fn config(&self) -> &WebConfig {
        &self.config
    }
}
