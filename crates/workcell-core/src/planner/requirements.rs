use crate::{
    ExecutionDemand, RequirementNecessity, ResourceRequirement, RetentionExpectation,
    StorageRequirement, WorkspaceAccess,
};

#[derive(Clone, Debug)]
pub(crate) struct RequirementAtom {
    pub(crate) kind: &'static str,
    pub(crate) key: String,
    pub(crate) necessity: RequirementNecessity,
    pub(crate) rule: MatchRule,
}

#[derive(Clone, Debug)]
pub(crate) enum MatchRule {
    Affordance(String),
    Connection(String),
    Exposure(String),
    Isolation(String),
    Capacity(ResourceRequirement),
    Storage(StorageRequirement),
}

pub(crate) fn atoms(demand: &ExecutionDemand) -> Vec<RequirementAtom> {
    let mut out = Vec::new();
    add_strings(
        &mut out,
        "affordance",
        demand.affordances.required.iter().map(|v| v.as_str()),
        RequirementNecessity::Required,
        MatchRule::Affordance,
    );
    add_strings(
        &mut out,
        "affordance",
        demand.affordances.preferred.iter().map(|v| v.as_str()),
        RequirementNecessity::Preferred,
        MatchRule::Affordance,
    );
    add_strings(
        &mut out,
        "affordance",
        demand.affordances.optional.iter().map(|v| v.as_str()),
        RequirementNecessity::Optional,
        MatchRule::Affordance,
    );
    add_storage(
        &mut out,
        &demand.storage.required,
        RequirementNecessity::Required,
    );
    add_storage(
        &mut out,
        &demand.storage.preferred,
        RequirementNecessity::Preferred,
    );
    add_storage(
        &mut out,
        &demand.storage.optional,
        RequirementNecessity::Optional,
    );
    add_strings(
        &mut out,
        "connectivity",
        demand.connectivity.required.iter().map(|v| v.as_str()),
        RequirementNecessity::Required,
        MatchRule::Connection,
    );
    add_strings(
        &mut out,
        "connectivity",
        demand.connectivity.preferred.iter().map(|v| v.as_str()),
        RequirementNecessity::Preferred,
        MatchRule::Connection,
    );
    add_strings(
        &mut out,
        "connectivity",
        demand.connectivity.optional.iter().map(|v| v.as_str()),
        RequirementNecessity::Optional,
        MatchRule::Connection,
    );
    add_strings(
        &mut out,
        "exposure",
        demand.exposure.required.iter().map(|v| v.as_str()),
        RequirementNecessity::Required,
        MatchRule::Exposure,
    );
    add_strings(
        &mut out,
        "exposure",
        demand.exposure.preferred.iter().map(|v| v.as_str()),
        RequirementNecessity::Preferred,
        MatchRule::Exposure,
    );
    add_strings(
        &mut out,
        "exposure",
        demand.exposure.optional.iter().map(|v| v.as_str()),
        RequirementNecessity::Optional,
        MatchRule::Exposure,
    );
    add_outputs(
        &mut out,
        demand.outputs.required.iter().map(|v| v.as_str()),
        RequirementNecessity::Required,
    );
    add_outputs(
        &mut out,
        demand.outputs.preferred.iter().map(|v| v.as_str()),
        RequirementNecessity::Preferred,
    );
    add_outputs(
        &mut out,
        demand.outputs.optional.iter().map(|v| v.as_str()),
        RequirementNecessity::Optional,
    );

    if let Some(workspace) = &demand.workspace {
        let key = match workspace.access {
            WorkspaceAccess::ReadOnly => "workspace:read-only",
            WorkspaceAccess::Writable => "workspace:writable",
        };
        out.push(atom(
            "workspace",
            key,
            RequirementNecessity::Required,
            MatchRule::Affordance(key.into()),
        ));
    }
    if let Some(runtime) = &demand.project_runtime {
        let key = format!("runtime-mode:{}", runtime.as_str());
        out.push(RequirementAtom {
            kind: "project-runtime",
            key: key.clone(),
            necessity: RequirementNecessity::Required,
            rule: MatchRule::Affordance(key),
        });
    }
    for resource in &demand.resources {
        out.push(RequirementAtom {
            kind: "resource",
            key: resource.key.clone(),
            necessity: RequirementNecessity::Required,
            rule: MatchRule::Capacity(resource.clone()),
        });
    }
    if let Some(scope) = &demand.persistence {
        let key = format!("persistence:{}", persistence_key(scope));
        out.push(RequirementAtom {
            kind: "persistence",
            key: key.clone(),
            necessity: RequirementNecessity::Required,
            rule: MatchRule::Affordance(key),
        });
    }
    if let Some(value) = &demand.isolation_trust {
        out.push(atom(
            "isolation-trust",
            value.as_str(),
            RequirementNecessity::Required,
            MatchRule::Isolation(value.as_str().into()),
        ));
    }
    match demand.retention {
        RetentionExpectation::Release => {}
        RetentionExpectation::Preserve => {
            out.push(retention("preserve", RequirementNecessity::Required))
        }
        RetentionExpectation::SuspendIfSupported => {
            out.push(retention("suspend", RequirementNecessity::Preferred))
        }
        RetentionExpectation::SnapshotIfSupported => {
            out.push(retention("snapshot", RequirementNecessity::Preferred))
        }
    }
    out
}

fn add_strings<'a>(
    out: &mut Vec<RequirementAtom>,
    kind: &'static str,
    values: impl Iterator<Item = &'a str>,
    necessity: RequirementNecessity,
    rule: fn(String) -> MatchRule,
) {
    for value in values {
        out.push(atom(kind, value, necessity, rule(value.into())));
    }
}

fn add_storage(
    out: &mut Vec<RequirementAtom>,
    values: &[StorageRequirement],
    necessity: RequirementNecessity,
) {
    for value in values {
        out.push(RequirementAtom {
            kind: "storage",
            key: value.logical_ref.clone(),
            necessity,
            rule: MatchRule::Storage(value.clone()),
        });
    }
}

fn add_outputs<'a>(
    out: &mut Vec<RequirementAtom>,
    values: impl Iterator<Item = &'a str>,
    necessity: RequirementNecessity,
) {
    for value in values {
        out.push(RequirementAtom {
            kind: "output",
            key: value.into(),
            necessity,
            rule: MatchRule::Affordance(format!("artifact-channel:{value}")),
        });
    }
}

fn atom(
    kind: &'static str,
    key: &str,
    necessity: RequirementNecessity,
    rule: MatchRule,
) -> RequirementAtom {
    RequirementAtom {
        kind,
        key: key.into(),
        necessity,
        rule,
    }
}

fn retention(value: &str, necessity: RequirementNecessity) -> RequirementAtom {
    let key = format!("retention:{value}");
    RequirementAtom {
        kind: "retention",
        key: key.clone(),
        necessity,
        rule: MatchRule::Affordance(key),
    }
}

fn persistence_key(scope: &crate::PersistenceScope) -> &'static str {
    match scope {
        crate::PersistenceScope::Ephemeral => "ephemeral",
        crate::PersistenceScope::TaskOrRun => "task-or-run",
        crate::PersistenceScope::Candidate => "candidate",
        crate::PersistenceScope::Project => "project",
        crate::PersistenceScope::Workcell => "workcell",
        crate::PersistenceScope::Factory => "factory",
        crate::PersistenceScope::External => "external",
    }
}
