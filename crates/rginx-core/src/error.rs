use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("address parse error: {0}")]
    AddrParse(#[from] std::net::AddrParseError),
    #[error("invalid uri: {0}")]
    InvalidUri(#[from] http::uri::InvalidUri),
    #[error("invalid status code: {0}")]
    InvalidStatusCode(#[from] http::status::InvalidStatusCode),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("server error: {0}")]
    Server(String),
}

pub type Result<T> = std::result::Result<T, Error>;
