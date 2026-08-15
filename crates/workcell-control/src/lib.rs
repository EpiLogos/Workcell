mod client;
pub mod codec;
mod service;

pub use client::{
    ControlClient, ControlClientError, ControlTransport, DirectTransport, LengthPrefixedTransport,
    TransportFailure, UnavailableTransport,
};
pub use service::ControlService;

pub const CONTROL_PROTOCOL_VERSION: &str = "workcell.control/v1";
