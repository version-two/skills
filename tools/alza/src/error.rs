use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    #[error("not an Alza product ID or product URL: {0}")]
    InvalidProduct(String),
    #[error("product not found at {url}: {message}")]
    ProductGone { message: String, url: String },
    #[error("HTTP {status} from {url}")]
    Http { status: u16, url: String },
    #[error("request to {url} failed: {source}")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("Alza API reported error {code} at {url}: {message}")]
    Api { code: i64, message: String, url: String },
    #[error("unexpected API response at {url}: {reason}")]
    Parse { url: String, reason: String },
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::InvalidQuery(_) => "invalid_query",
            Error::InvalidProduct(_) => "invalid_product",
            Error::ProductGone { .. } => "product_gone",
            Error::Http { .. } => "http_error",
            Error::Transport { .. } => "transport_error",
            Error::Api { .. } => "api_error",
            Error::Parse { .. } => "parse_error",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Error::InvalidQuery(_) | Error::InvalidProduct(_) => 2,
            Error::ProductGone { .. } => 3,
            Error::Http { .. } | Error::Transport { .. } | Error::Api { .. } => 4,
            Error::Parse { .. } => 5,
        }
    }
}
