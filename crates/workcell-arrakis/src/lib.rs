//! Arrakis-backed Workcell execution provider.
//!
//! The adapter invokes the pinned first-party Arrakis client, which speaks the
//! upstream REST API. MicroVM lifecycle remains owned by Arrakis; Workcell
//! exposes only provider-neutral execution demand and material provenance.

mod command;
mod execution;
mod support;

pub use command::*;
pub use execution::*;
pub use support::{
    ARRAKIS_API_VERSION, ARRAKIS_INTEGRATION_SEAM, ARRAKIS_LICENSE, ARRAKIS_SOURCE_REVISION,
};

pub(crate) use support::{source_metadata, stable_key, status_health};
