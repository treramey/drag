use std::error::Error as StdError;
use std::io;

use reqwest::StatusCode;
use thiserror::Error;

pub(crate) const EXIT_FAILURE: u8 = 1;
pub(crate) const EXIT_USAGE: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteService {
    Jira,
    Tempo,
    Unknown,
}

impl RemoteService {
    pub(crate) fn from_url(url: &url::Url) -> Self {
        match url.host_str() {
            Some("api.tempo.io") => Self::Tempo,
            Some(host) if host.ends_with(".atlassian.net") => Self::Jira,
            _ => Self::Unknown,
        }
    }
}

impl std::fmt::Display for RemoteService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Jira => "Jira",
            Self::Tempo => "Tempo",
            Self::Unknown => "remote service",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteErrorKind {
    Authentication,
    InvalidResponse,
    Rejected,
}

#[derive(Debug, Error)]
#[error("{service} {message}")]
pub(crate) struct RemoteError {
    pub(crate) service: RemoteService,
    pub(crate) status: Option<StatusCode>,
    pub(crate) kind: RemoteErrorKind,
    pub(crate) message: String,
}

#[derive(Debug, Error)]
pub(crate) enum CliError {
    #[error(transparent)]
    Core(#[from] drag::Error),
    #[error("{0}")]
    InvalidInput(String),
    #[error("drag is not configured: {0}")]
    NotConfigured(String),
    #[error("configuration error: {message}")]
    Config {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    #[error("API request failed: {0}")]
    Api(String),
    #[error("API request failed: {0}")]
    Remote(RemoteError),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("generated completion output was not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

impl CliError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::Core(error) => error.code(),
            Self::InvalidInput(_) => "invalid_input",
            Self::NotConfigured(_) => "not_configured",
            Self::Config { .. } => "config_error",
            Self::Api(_) | Self::Remote(_) => "api_error",
            Self::Http(_) => "http_error",
            Self::Url(_) => "invalid_url",
            Self::Json(_) => "invalid_json",
            Self::Io(_) => "io_error",
            Self::Utf8(_) => "encoding_error",
        }
    }

    pub(crate) const fn exit_code(&self) -> u8 {
        match self {
            Self::Core(_)
            | Self::InvalidInput(_)
            | Self::NotConfigured(_)
            | Self::Json(_)
            | Self::Url(_) => EXIT_USAGE,
            Self::Config { .. }
            | Self::Api(_)
            | Self::Remote(_)
            | Self::Http(_)
            | Self::Io(_)
            | Self::Utf8(_) => EXIT_FAILURE,
        }
    }

    pub(crate) fn is_authentication(&self) -> bool {
        matches!(self, Self::Remote(error) if error.kind == RemoteErrorKind::Authentication)
    }

    #[cfg(test)]
    pub(crate) fn authentication(service: RemoteService, message: impl Into<String>) -> Self {
        Self::Remote(RemoteError {
            service,
            status: None,
            kind: RemoteErrorKind::Authentication,
            message: message.into(),
        })
    }
}

impl From<drag::pagination::PaginationError> for CliError {
    fn from(error: drag::pagination::PaginationError) -> Self {
        Self::InvalidInput(error.to_string())
    }
}
