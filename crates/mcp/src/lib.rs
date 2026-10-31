use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ApiResponseEnvelope<T> {
    pub success: bool,
    pub data: Option<T>,
    pub message: Option<String>,
}

pub mod api_auth;
pub mod task_server;

#[cfg(test)]
pub(crate) fn install_rustls_provider_for_tests() {
    static RUSTLS_PROVIDER: std::sync::Once = std::sync::Once::new();
    RUSTLS_PROVIDER.call_once(|| {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    });
}
