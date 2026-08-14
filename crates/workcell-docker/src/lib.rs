//! Docker-backed Workcell provider adapters.
//!
//! Docker remains below the provider-neutral Workcell contract. Semantic
//! clients express affordances, logical connectivity, persistence and runtime
//! modes; this crate translates those material requirements into Docker Engine
//! and Docker Compose CLI operations and returns provider provenance.

mod command;
mod execution;
mod runtime;
mod support;

pub use command::*;
pub use execution::*;
pub use runtime::*;
pub use support::{
    DOCKER_COMPOSE_SOURCE_PIN, DOCKER_ENGINE_SOURCE_PIN, DOCKER_INTEGRATION_SEAM,
};

pub(crate) use support::*;
