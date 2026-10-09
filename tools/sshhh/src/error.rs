use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Usage(String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::Usage(_) => "usage",
        }
    }
}
