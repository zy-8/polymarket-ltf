use std::error::Error as StdError;

pub type Result<T> = std::result::Result<T, PolyfillError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamErrorKind {
    ConnectionLost,
}

#[derive(Debug, thiserror::Error)]
pub enum PolyfillError {
    #[error("{message}")]
    Validation { message: String },
    #[error("{message}")]
    Internal { message: String },
    #[error("{message}")]
    Parse {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    #[error("{message}")]
    Stream {
        message: String,
        kind: StreamErrorKind,
    },
}

impl PolyfillError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    pub fn internal_simple(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    pub fn parse(
        message: impl Into<String>,
        source: Option<Box<dyn StdError + Send + Sync>>,
    ) -> Self {
        Self::Parse {
            message: message.into(),
            source,
        }
    }

    pub fn stream(message: impl Into<String>, kind: StreamErrorKind) -> Self {
        Self::Stream {
            message: message.into(),
            kind,
        }
    }
}
