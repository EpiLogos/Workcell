mod external_service;
mod host;
mod local;
mod profile;
mod reference_services;
mod runtime;
mod secret;
mod service;
mod support;

pub use external_service::{
    ExternalManagedService, ExternalManagedServiceProvider, ExternalServiceAcquisition,
    ExternalServiceCommand,
};
pub use host::{HostProcessExecutionProvider, HostProcessOperationGrant};
pub use local::{CollapsedLocalConfig, CollapsedLocalWorkcell};
pub use profile::*;
pub use reference_services::{
    hermes_gateway_service, openclaw_gateway_service, HERMES_MANAGEMENT_SOURCE,
    HERMES_SOURCE_REVISION, OPENCLAW_MANAGEMENT_SOURCE, OPENCLAW_SOURCE_REVISION,
};
pub use runtime::{ReferenceProjectRuntimeProvider, RuntimeMode};
pub use secret::{
    run_with_secret_env, run_with_secret_file, run_with_secret_pipe, MaterialisedChild,
};
pub use service::{
    ManagedHostService, ManagedHostServiceProvider, StaticService, StaticServiceProvider,
    TcpEndpointProbe,
};
