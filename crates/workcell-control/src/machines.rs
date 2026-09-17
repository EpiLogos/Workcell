//! Operator-declared remote machines: the native "add a machine" setup.
//!
//! A declaration names a serving cell — a label, its control endpoint, the
//! operations it is expected to serve, and the credential *reference*. The
//! credential material never lives here: the declaration carries a secret
//! reference (`keychain://…`, `linux-secret-service://…`), and `workcell
//! connect` resolves the material from this cell's origin secret source at
//! connect time. No second connection truth, no plaintext in state.
//!
//! The store is one JSON file under the workcell state root
//! (`<state-root>/machines.json`), following the same law as the other
//! registries: a missing file is an empty registry, an unreadable or invalid
//! file is named unavailability, and a duplicate label is a named conflict.

use std::{
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::{json, Value};

pub const MACHINES_FILE: &str = "machines.json";
pub const MACHINES_SCHEMA: &str = "workcell.remote-machines/v1";

/// Secret-reference schemes a machine declaration may name. The declaration
/// is a location, never a value: anything that does not start with one of
/// these schemes is refused rather than stored.
pub const MACHINE_CREDENTIAL_SCHEMES: [&str; 2] = ["keychain://", "linux-secret-service://"];

/// One declared remote machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteMachineDeclaration {
    pub label: String,
    pub endpoint: String,
    pub credential_ref: Option<String>,
    pub operations: Vec<String>,
    pub note: Option<String>,
}

impl RemoteMachineDeclaration {
    pub fn to_json(&self) -> Value {
        json!({
            "label": self.label,
            "endpoint": self.endpoint,
            "credential_ref": self.credential_ref,
            "operations": self.operations,
            "note": self.note,
        })
    }

    pub fn from_json(value: &Value) -> Result<Self> {
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::InvalidDemand("each declared machine must be a JSON object".into())
        })?;
        let field = |name: &str| -> Result<String> {
            object
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    WorkcellError::InvalidDemand(format!(
                        "declared machine field `{name}` must be a non-empty string"
                    ))
                })
        };
        let operations = object
            .get("operations")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.as_str().map(str::to_owned).ok_or_else(|| {
                            WorkcellError::InvalidDemand(
                                "declared machine `operations` must contain strings".into(),
                            )
                        })
                    })
                    .collect::<Result<Vec<String>>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            label: field("label")?,
            endpoint: field("endpoint")?,
            credential_ref: object
                .get("credential_ref")
                .and_then(Value::as_str)
                .map(str::to_owned),
            operations,
            note: object
                .get("note")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

pub struct RemoteMachineRegistry {
    path: PathBuf,
}

impl RemoteMachineRegistry {
    pub fn new(state_root: impl AsRef<Path>) -> Self {
        Self {
            path: state_root.as_ref().join(MACHINES_FILE),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn empty_file() -> Value {
        json!({
            "schema": MACHINES_SCHEMA,
            "machines": [],
        })
    }

    /// Load the registry. A missing file is an empty registry; an unreadable
    /// or invalid file is named unavailability, never an empty result.
    pub fn load(&self) -> Result<Value> {
        if !self.path.exists() {
            return Ok(Self::empty_file());
        }
        let text = fs::read_to_string(&self.path).map_err(|error| {
            WorkcellError::Unavailable(format!(
                "machine registry at {} could not be read: {error}",
                self.path.display()
            ))
        })?;
        let value: Value = serde_json::from_str(&text).map_err(|error| {
            WorkcellError::Unavailable(format!(
                "machine registry at {} is not valid JSON: {error}",
                self.path.display()
            ))
        })?;
        if value.get("schema").and_then(Value::as_str) != Some(MACHINES_SCHEMA) {
            return Err(WorkcellError::Unavailable(format!(
                "machine registry at {} does not carry schema {MACHINES_SCHEMA}",
                self.path.display()
            )));
        }
        Ok(value)
    }

    fn save(&self, value: &Value) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "could not create {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let text = serde_json::to_string_pretty(value)
            .map_err(|error| WorkcellError::OperationFailed(format!("encode machines: {error}")))?;
        fs::write(&self.path, text).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "could not write {}: {error}",
                self.path.display()
            ))
        })
    }

    pub fn list(&self) -> Result<Vec<RemoteMachineDeclaration>> {
        let file = self.load()?;
        let mut machines = Vec::new();
        if let Some(entries) = file.get("machines").and_then(Value::as_array) {
            for entry in entries {
                machines.push(RemoteMachineDeclaration::from_json(entry)?);
            }
        }
        machines.sort_by(|a, b| a.label.cmp(&b.label));
        Ok(machines)
    }

    pub fn get(&self, label: &str) -> Result<Option<RemoteMachineDeclaration>> {
        Ok(self
            .list()?
            .into_iter()
            .find(|machine| machine.label == label))
    }

    /// Record a machine. An existing label is a named conflict, never a
    /// silent overwrite — remove first to replace a declaration.
    pub fn add(&self, declaration: RemoteMachineDeclaration) -> Result<()> {
        if declaration.label.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "machine label must not be empty".into(),
            ));
        }
        if declaration.endpoint.trim().is_empty()
            || declaration.endpoint.contains(char::is_whitespace)
        {
            return Err(WorkcellError::InvalidDemand(
                "machine endpoint must be a non-empty HOST:PORT without whitespace".into(),
            ));
        }
        if let Some(reference) = &declaration.credential_ref {
            if !MACHINE_CREDENTIAL_SCHEMES
                .iter()
                .any(|scheme| reference.starts_with(scheme))
            {
                return Err(WorkcellError::InvalidDemand(format!(
                    "machine credential_ref must be a secret reference ({}), never material",
                    MACHINE_CREDENTIAL_SCHEMES.join(" or ")
                )));
            }
        }
        let mut file = self.load()?;
        let machines = file
            .as_object_mut()
            .and_then(|file| file.get_mut("machines"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                WorkcellError::Unavailable("machine registry has no `machines` array".into())
            })?;
        let exists = machines.iter().any(|entry| {
            entry.get("label").and_then(Value::as_str) == Some(declaration.label.as_str())
        });
        if exists {
            return Err(WorkcellError::InvalidDemand(format!(
                "machine `{}` is already declared; remove it first to replace the declaration",
                declaration.label
            )));
        }
        machines.push(declaration.to_json());
        self.save(&file)
    }

    pub fn remove(&self, label: &str) -> Result<RemoteMachineDeclaration> {
        let mut file = self.load()?;
        let machines = file
            .as_object_mut()
            .and_then(|file| file.get_mut("machines"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                WorkcellError::Unavailable("machine registry has no `machines` array".into())
            })?;
        let position = machines
            .iter()
            .position(|entry| entry.get("label").and_then(Value::as_str) == Some(label))
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(format!("no machine `{label}` is declared"))
            })?;
        let removed = RemoteMachineDeclaration::from_json(&machines.remove(position))?;
        self.save(&file)?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> (RemoteMachineRegistry, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "wck-machines-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (RemoteMachineRegistry::new(&root), root)
    }

    fn declaration(label: &str) -> RemoteMachineDeclaration {
        RemoteMachineDeclaration {
            label: label.to_owned(),
            endpoint: "100.92.62.101:7777".to_owned(),
            credential_ref: Some("keychain://workcell-connection/omarchy".to_owned()),
            operations: vec!["status".to_owned(), "prepare".to_owned()],
            note: Some("omarchy workstation".to_owned()),
        }
    }

    #[test]
    fn machines_round_trip_and_list_sorted() {
        let (registry, root) = registry();
        registry.add(declaration("zeta")).unwrap();
        let mut other = declaration("alpha");
        other.endpoint = "10.0.0.9:7777".to_owned();
        registry.add(other).unwrap();

        let listed = registry.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].label, "alpha");
        assert_eq!(
            registry.get("zeta").unwrap().unwrap().endpoint,
            "100.92.62.101:7777"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn duplicate_label_is_a_named_conflict() {
        let (registry, root) = registry();
        registry.add(declaration("omarchy")).unwrap();
        let error = registry.add(declaration("omarchy")).unwrap_err();
        assert!(error.to_string().contains("already declared"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn credential_ref_must_be_a_reference_not_material() {
        let (registry, root) = registry();
        let mut declaration = declaration("omarchy");
        declaration.credential_ref = Some("wck_raw_material".to_owned());
        let error = registry.add(declaration).unwrap_err();
        assert!(error.to_string().contains("secret reference"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_machine_is_named_and_remove_is_symmetric() {
        let (registry, root) = registry();
        assert!(registry.get("nonesuch").unwrap().is_none());
        registry.add(declaration("omarchy")).unwrap();
        let removed = registry.remove("omarchy").unwrap();
        assert_eq!(removed.label, "omarchy");
        assert!(registry.get("omarchy").unwrap().is_none());
        let error = registry.remove("omarchy").unwrap_err();
        assert!(error.to_string().contains("no machine"));
        std::fs::remove_dir_all(&root).ok();
    }
}
