use thiserror::Error;

/// The one error type used across the crate.
#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("wrong kind: expected {expected}, found {found}")]
    WrongKind { expected: String, found: String },
    #[error("bad model output: {0}")]
    BadModelOutput(String),
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("effect violation: {0}")]
    EffectViolation(String),
    #[error("not verified: {0}")]
    NotVerified(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("plug-in error: {0}")]
    Plug(String),
    /// A model reply is waiting on someone else (the handoff provider). Not a
    /// failure of the work: answer the pending prompts and run again.
    #[error("pending: {0}")]
    Pending(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<serde_json::Error> for Error {
    // DECISION: the error enum is closed (section 3), so JSON failures in stored
    // data are reported as Io with kind InvalidData.
    fn from(e: serde_json::Error) -> Self {
        Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
