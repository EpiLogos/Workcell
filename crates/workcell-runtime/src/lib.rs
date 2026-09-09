mod external_service;
mod host;
mod instance_projection;
mod instance_registry;
mod instance_scan;
mod local;
mod profile;
mod reference_services;
mod resource_usage;
mod runtime;
mod secret;
mod service;
mod support;

pub use external_service::{
    ExternalManagedService, ExternalManagedServiceProvider, ExternalServiceAcquisition,
    ExternalServiceCommand,
};
pub use host::{HostProcessExecutionProvider, HostProcessOperationGrant};
pub use instance_projection::{
    is_projection_candidate, project_instance, project_instance_live, project_record,
    projection_candidates, projection_report_json, ProjectionReport,
};
pub use instance_registry::{
    build_instance_record, by_slug, identity_hash, seam, sort_by_reference,
    validate_instance_record, InstanceObservation, InstanceRegistry, RegisterOutcome,
    EVIDENCE_DECLARED_UNVERIFIED, EVIDENCE_GATEWAY_CONFIRMED, EVIDENCE_LIVE_PID,
    HARNESS_INSTANCE_SCHEMA, LIVENESS_LIVE, LIVENESS_STALE, REGISTRY_SCHEMA,
};
pub use instance_scan::{
    gateway_answering, read_pid_table, reconcile, report_json, scan_inputs_live, scan_live,
    InstanceConflict, ObservedInstance, ScanInputs, ScanReport, ScanTransitions, PID_ALIASES,
    STALE_AFTER_MISSED_SCANS,
};
pub use local::{CollapsedLocalConfig, CollapsedLocalWorkcell};
pub use profile::*;
pub use reference_services::{
    aikit_gateway_service, hermes_gateway_service, openclaw_gateway_service,
    AIKIT_GATEWAY_APPLICATION_PROTOCOL, AIKIT_GATEWAY_MANAGEMENT_SOURCE,
    AIKIT_GATEWAY_SOURCE_REVISION, HERMES_MANAGEMENT_SOURCE, HERMES_SOURCE_REVISION,
    OPENCLAW_MANAGEMENT_SOURCE, OPENCLAW_SOURCE_REVISION,
};
pub use resource_usage::{
    observe_resource_usage, validate_resource_usage, ResourceUsageReport, DEFAULT_INTERVAL,
    MAX_INTERVAL, RESOURCE_USAGE_SCHEMA,
};
pub use runtime::{ReferenceProjectRuntimeProvider, RuntimeMode};
pub use secret::{
    run_with_secret_env, run_with_secret_file, run_with_secret_pipe, MaterialisedChild,
};
pub use service::{
    ManagedHostService, ManagedHostServiceProvider, StaticService, StaticServiceProvider,
    TcpEndpointProbe,
};
