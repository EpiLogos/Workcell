use epilogos_workcell_core::{PersistenceScope, WorkspaceAccess, WorkcellError};
use epilogos_workcell_interop::{
    validate_factory_identity, FactoryIdentityRole, WorkcellInteropFixture,
    FACTORY_INTEROP_FIXTURE_VERSION, FACTORY_INTEROP_PROTOCOL_VERSION,
    FACTORY_INTEROP_SOURCE_REVISION,
};
use serde_json::Value;

const FIXTURE: &str = include_str!("../fixtures/factory-interop-v1.json");

#[test]
fn shared_factory_fixture_round_trips_and_adapts_without_identity_collapse() {
    let fixture = WorkcellInteropFixture::parse(FIXTURE).unwrap();
    assert_eq!(fixture.fixture_version(), FACTORY_INTEROP_FIXTURE_VERSION);
    assert_eq!(fixture.protocol_version(), FACTORY_INTEROP_PROTOCOL_VERSION);
    assert!(fixture
        .anti_fixture_ids()
        .iter()
        .any(|id| id == "provider-as-project"));

    let demand = fixture.to_workcell_execution_demand().unwrap();
    assert_eq!(demand.demand_ref.as_str(), "factory:execution-demand:d-1");
    assert_eq!(
        demand.subjects.get("project").map(|item| item.as_str()),
        Some("factory:project:factory")
    );
    assert_eq!(
        demand.subjects.get("run").map(|item| item.as_str()),
        Some("factory:run:r-1")
    );
    assert_eq!(
        demand.subjects.get("candidate").map(|item| item.as_str()),
        Some("factory:candidate:c-1")
    );
    assert_eq!(demand.affordances.required[0].as_str(), "shell");
    assert!(demand
        .affordances
        .preferred
        .iter()
        .any(|item| item.as_str() == "isolated-execution"));
    assert_eq!(demand.resources[0].key, "cpu");
    assert_eq!(demand.resources[0].minimum, Some(2));
    assert_eq!(demand.resources[1].key, "memory");
    assert_eq!(demand.resources[1].unit.as_deref(), Some("GiB"));
    assert_eq!(demand.persistence, Some(PersistenceScope::Candidate));
    assert_eq!(
        demand.workspace.as_ref().map(|workspace| &workspace.access),
        Some(&WorkspaceAccess::Writable)
    );
    assert_eq!(demand.isolation_trust, None);
    assert_eq!(
        demand
            .extensions
            .get("factory.isolationRequirement")
            .map(String::as_str),
        Some("strong-preferred")
    );

    let original: Value = serde_json::from_str(FIXTURE).unwrap();
    let round_tripped: Value = serde_json::from_str(&fixture.round_trip_json().unwrap()).unwrap();
    assert_eq!(round_tripped, original);

    let report = fixture.consumption_report();
    assert_eq!(report.fixture_version, FACTORY_INTEROP_FIXTURE_VERSION);
    assert_eq!(report.protocol_version, FACTORY_INTEROP_PROTOCOL_VERSION);
    assert_eq!(report.source_revision, FACTORY_INTEROP_SOURCE_REVISION);
    assert_eq!(report.workcell_ref, "workcell:ubuntu-worker-1");
    assert_eq!(report.binding_logical_ref, "state:graph");
    assert_eq!(report.provider_ref, "workcell-provider:docker");
}

#[test]
fn incompatible_fixture_and_protocol_versions_fail_explicitly() {
    let mut fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    fixture["fixtureVersion"] = Value::String("factory.interop-fixtures/v2".into());
    assert!(matches!(
        WorkcellInteropFixture::parse(&fixture.to_string()),
        Err(WorkcellError::Unsupported(_))
    ));

    let mut fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    fixture["contract"]["contractVersion"] = Value::String("factory.interop/v2".into());
    assert!(matches!(
        WorkcellInteropFixture::parse(&fixture.to_string()),
        Err(WorkcellError::Unsupported(_))
    ));
}

#[test]
fn provider_material_changes_do_not_mutate_semantic_execution_demand() {
    let baseline = WorkcellInteropFixture::parse(FIXTURE)
        .unwrap()
        .to_workcell_execution_demand()
        .unwrap();
    let mut changed: Value = serde_json::from_str(FIXTURE).unwrap();
    changed["contract"]["binding"]["providerRef"] = Value::String("workcell-provider:arrakis".into());
    changed["contract"]["binding"]["concreteRef"] = Value::String("resource:vm:microvm-22".into());
    let substituted = WorkcellInteropFixture::parse(&changed.to_string())
        .unwrap()
        .to_workcell_execution_demand()
        .unwrap();
    assert_eq!(baseline, substituted);
}

#[test]
fn provider_and_material_ids_cannot_masquerade_as_factory_semantic_refs() {
    let material_ids = [
        "provider:github:EpiLogos/Workcell",
        "workcell-provider:docker",
        "worktree:/tmp/run-1",
        "container:deadbeef",
        "vm:arrakis-22",
        "process:4412",
    ];
    for role in [
        FactoryIdentityRole::Project,
        FactoryIdentityRole::Run,
        FactoryIdentityRole::Candidate,
        FactoryIdentityRole::Agent,
    ] {
        for material_id in material_ids {
            assert!(validate_factory_identity(role, material_id).is_err());
        }
    }

    assert!(validate_factory_identity(FactoryIdentityRole::Project, "factory:project:factory").is_ok());
    assert!(validate_factory_identity(FactoryIdentityRole::Run, "factory:run:r-1").is_ok());
    assert!(validate_factory_identity(FactoryIdentityRole::Candidate, "factory:candidate:c-1").is_ok());
    assert!(validate_factory_identity(FactoryIdentityRole::Agent, "factory:agent:builder").is_ok());
    assert!(validate_factory_identity(FactoryIdentityRole::Project, "provider:github:EpiLogos/agent-system-design").is_err());
    assert!(validate_factory_identity(FactoryIdentityRole::Agent, "model:gpt-5.6-sol").is_err());
    assert!(validate_factory_identity(FactoryIdentityRole::Agent, "factory:agent-session:s-1").is_err());
}

#[test]
fn workcell_core_has_no_factory_fixture_or_interop_runtime_dependency() {
    let manifest = include_str!("../../workcell-core/Cargo.toml").to_ascii_lowercase();
    assert!(!manifest.contains("workcell-interop"));
    assert!(!manifest.contains("factory"));
    assert!(!manifest.contains("serde_json"));

    let demand_source = include_str!("../../workcell-core/src/demand.rs").to_ascii_lowercase();
    assert!(!demand_source.contains("factory:project:"));
    assert!(!demand_source.contains("factory:run:"));
    assert!(!demand_source.contains("factory:candidate:"));
}
