mod client;
pub mod codec;
mod network;
mod service;

pub use client::{
    ControlClient, ControlClientError, ControlTransport, DirectTransport, LengthPrefixedTransport,
    TransportFailure, UnavailableTransport,
};
pub use network::{TcpControlServer, TcpControlTransport};
pub use service::ControlService;

pub const CONTROL_PROTOCOL_VERSION: &str = "workcell.control/v1";
