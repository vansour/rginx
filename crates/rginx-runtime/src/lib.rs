pub mod acme;
pub mod admin;
pub mod bootstrap;
mod cache;
mod health;
mod ocsp;
pub mod reload;
pub mod restart;
pub mod shutdown;
pub mod state;

pub use bootstrap::run;

#[cfg(test)]
#[ctor::ctor]
fn install_test_crypto_provider() {
    rginx_http::install_default_crypto_provider();
}
