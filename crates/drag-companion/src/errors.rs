use crate::*;

#[derive(Debug, Error)]
pub(crate) enum CompanionError {
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize result: {0}")]
    Serialize(serde_json::Error),
    #[error("invalid journal event on line {line}: {reason}")]
    InvalidJournal { line: usize, reason: String },
    #[error("sqlite store error: {0}")]
    Store(#[from] rusqlite::Error),
    #[error("failed to open {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid Claude hook payload: {0}")]
    InvalidClaudeHook(String),
    #[error("proposal adapter rejected response: {0}")]
    Proposal(String),
    #[error("drag reconciliation {kind}: {message}")]
    DragReconcile {
        kind: ReconcileErrorKind,
        message: String,
    },
    #[error(
        "run already owned for Tempo account {account} on {date} by {owner} until {expires_at}"
    )]
    RunOwned {
        account: String,
        date: NaiveDate,
        owner: String,
        expires_at: String,
    },
    #[error("phase {0} is not retryable")]
    NotRetryable(&'static str),
    #[error("blocked before mutation; resume will not enter submission")]
    BlockedBeforeMutation,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ReconcileErrorKind {
    IncompleteRead,
    SchemaIncompatibility,
    DefiniteFailure,
    TransportAmbiguity,
}

impl std::fmt::Display for ReconcileErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::IncompleteRead => "incomplete_read",
            Self::SchemaIncompatibility => "schema_incompatibility",
            Self::DefiniteFailure => "definite_failure",
            Self::TransportAmbiguity => "transport_ambiguity",
        })
    }
}
