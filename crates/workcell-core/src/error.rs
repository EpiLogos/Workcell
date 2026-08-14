use std::{error::Error, fmt};

pub type Result<T> = std::result::Result<T, WorkcellError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkcellError {
    InvalidDemand(String),
    UnsatisfiedDemand(String),
    Unavailable(String),
    Degraded(String),
    OperationFailed(String),
    CleanupFailed(String),
    ReconciliationFailed(String),
    NotFound(String),
    Unsupported(String),
}

impl fmt::Display for WorkcellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, detail) = match self {
            Self::InvalidDemand(v) => ("invalid demand", v),
            Self::UnsatisfiedDemand(v) => ("unsatisfied demand", v),
            Self::Unavailable(v) => ("unavailable", v),
            Self::Degraded(v) => ("degraded", v),
            Self::OperationFailed(v) => ("operation failed", v),
            Self::CleanupFailed(v) => ("cleanup failed", v),
            Self::ReconciliationFailed(v) => ("reconciliation failed", v),
            Self::NotFound(v) => ("not found", v),
            Self::Unsupported(v) => ("unsupported", v),
        };
        write!(f, "{kind}: {detail}")
    }
}

impl Error for WorkcellError {}
