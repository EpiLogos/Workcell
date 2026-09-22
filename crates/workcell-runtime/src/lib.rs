mod write_boundary;
pub use write_boundary::{
    write_boundary_capabilities, PreparedWriteBoundary, WriteBoundaryRequirements,
    WRITE_BOUNDARY_COVERAGE, WRITE_BOUNDARY_SCHEMA, WRITE_BOUNDARY_UNCOVERED,
};
mod directory_storage;
mod material_path;
pub use directory_storage::{
    read_directory_storage, DirectoryStorage, DirectoryStorageProvider, DIRECTORY_STORAGE_FILE,
    DIRECTORY_STORAGE_PROVIDER_REF, DIRECTORY_STORAGE_SCHEMA,
};
mod external_service;
mod host;
mod instance_projection;
mod instance_registry;
mod instance_scan;
mod local;
mod place;
mod place_scan;
mod profile;
mod reference_services;
mod resource_usage;
mod runtime;
mod secret;
mod service;
mod service_declaration;
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
    build_instance_record, by_slug, identity_hash, recorded_start_marker, seam, sort_by_reference,
    validate_instance_record, InstanceObservation, InstanceRegistry, ProcessExecution,
    RegisterOutcome, EVIDENCE_DECLARED_UNVERIFIED, EVIDENCE_GATEWAY_CONFIRMED, EVIDENCE_LIVE_PID,
    HARNESS_INSTANCE_SCHEMA, LIVENESS_LIVE, LIVENESS_STALE, REGISTRY_SCHEMA,
};
pub use instance_scan::{
    gateway_answering, read_pid_table, reconcile, report_json, scan_inputs_live, scan_live,
    GenerationReplacement, InstanceConflict, ObservedInstance, ObservedProcess, ScanInputs,
    ScanReport, ScanTransitions, PID_ALIASES, STALE_AFTER_MISSED_SCANS,
};
pub use local::{
    CollapsedLocalConfig, CollapsedLocalWorkcell, ExternalExecutionProviderFactory,
    ServiceDeclarationSource, MANAGED_SERVICE_PROVIDER_REF, OPENSANDBOX_PROVIDER_REF,
    TARGET_SERVICE_PROVIDER_REF,
};
pub use place::{
    decide_place_release, herdr_place_ref, parse_place_ref, release_place_live, request_place_live,
    request_tmux_place, tmux_place_ref, validate_place_name, PlaceGrant, PlacePolicy, PlaceRefusal,
    PlaceReleaseDecision, PlaceReleaseDemand, ProviderSnapshot, PLACE_GRANT_VERSION,
    PLACE_PROVIDER_CLOSE_VERSION, REFUSAL_PROVIDER_CLOSE_REFUSED,
};
pub use place_scan::{
    assemble_census, census_json, classify_command, format_rfc3339_utc, join_process_evidence,
    machine_name, parse_herdr_pane_list, parse_herdr_process_info, parse_herdr_workspace_list,
    parse_tmux_list_panes, scan_places_live, utc_now_rfc3339, HerdrPaneRow, HerdrProcessInfo,
    HerdrWorkspaceRow, PaneObservation, PlaceCensus, PlaceReuseFinding, ProviderCensus,
    TmuxPaneRow, HERDR_PROVIDER, PLACE_CENSUS_VERSION, PLACE_CLASS_HARNESS, PLACE_CLASS_OTHER,
    PLACE_CLASS_SELF, PLACE_CLASS_UNOBSERVED, PLACE_EVIDENCE_PROVIDER_FOREGROUND,
    PLACE_EVIDENCE_PS_COMM, PLACE_PROVIDER_ABSENT, PLACE_PROVIDER_CLI_UNPARSED,
    PLACE_PROVIDER_ERROR, PLACE_PROVIDER_NO_SERVER, PLACE_PROVIDER_OK,
    PLACE_PROVIDER_SERVER_UNREACHABLE, SELF_BINARY_STEMS, TMUX_PANE_FORMAT, TMUX_PROVIDER,
};
pub use profile::*;
pub use reference_services::{
    aikit_gateway_service, hermes_gateway_service, openclaw_gateway_service,
    redis_now_config_policy, redis_now_service,
    AIKIT_GATEWAY_APPLICATION_PROTOCOL, AIKIT_GATEWAY_MANAGEMENT_SOURCE,
    AIKIT_GATEWAY_SOURCE_REVISION, HERMES_MANAGEMENT_SOURCE, HERMES_SOURCE_REVISION,
    OPENCLAW_MANAGEMENT_SOURCE, OPENCLAW_SOURCE_REVISION, REDIS_NOW_MANAGEMENT_SOURCE,
    REDIS_NOW_MINIMUM_SERIES,
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
    HostLifetime, ManagedHostService, ManagedHostServiceProvider, StaticService,
    StaticServiceProvider, TcpEndpointProbe,
};
pub use service_declaration::{
    default_service_declaration_path, parse_service_declarations, read_service_declarations,
    read_state_root_service_declarations, DeclaredServices, ServiceLifetime,
    SERVICE_DECLARATION_FILE, SERVICE_DECLARATION_SCHEMA,
};

mod bounded_process;
pub use bounded_process::{run_bounded_process, BoundedProcessOutput};
