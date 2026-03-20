use tracing_subscriber::fmt;
use tracing_subscriber::EnvFilter;

pub fn init_logging() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rginx_core=info,rginx_http=info"));

    fmt().with_env_filter(filter).with_target(false).compact().try_init()
}
