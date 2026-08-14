//! Provider-neutral material execution contracts for EpiLogos Workcell.
//!
//! This crate owns Workcell material concepts only. Semantic clients retain
//! ownership of Project, Run, Candidate, Agent, Context and other identities.

mod api;
mod contract;
mod demand;
mod error;
mod refs;

pub use api::WorkcellControlPlane;
pub use contract::*;
pub use demand::*;
pub use error::{Result, WorkcellError};
pub use refs::*;
