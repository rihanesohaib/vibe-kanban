use std::sync::OnceLock;

/// Runtime information about the local server.
#[derive(Clone)]
pub struct ClientInfo {
    preview_proxy_port: OnceLock<u16>,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientInfo {
    pub fn new() -> Self {
        Self {
            preview_proxy_port: OnceLock::new(),
        }
    }

    pub fn set_preview_proxy_port(&self, port: u16) -> Result<(), String> {
        self.preview_proxy_port
            .set(port)
            .map_err(|_| "preview proxy port already set".to_string())
    }

    pub fn get_preview_proxy_port(&self) -> Option<u16> {
        self.preview_proxy_port.get().copied()
    }
}
