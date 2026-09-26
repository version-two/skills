use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    #[error("not a bazos.sk / bazos.cz ad URL: {0}")]
    InvalidAdUrl(String),
    #[error("ad not found or removed (HTTP {status}): {url}")]
    AdGone { status: u16, url: String },
    #[error("HTTP {status} from {url}")]
    Http { status: u16, url: String },
    #[error("request to {url} failed: {source}")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("unexpected page structure at {url}: {reason}")]
    Parse { url: String, reason: String },
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::InvalidQuery(_) => "invalid_query",
            Error::InvalidAdUrl(_) => "invalid_ad_url",
            Error::AdGone { .. } => "ad_gone",
            Error::Http { .. } => "http_error",
            Error::Transport { .. } => "transport_error",
            Error::Parse { .. } => "parse_error",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Error::InvalidQuery(_) | Error::InvalidAdUrl(_) => 2,
            Error::AdGone { .. } => 3,
            Error::Http { .. } | Error::Transport { .. } => 4,
            Error::Parse { .. } => 5,
        }
    }
}
