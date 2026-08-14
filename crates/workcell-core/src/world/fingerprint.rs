use crate::{BindingRef, DemandRef, ProviderRef, Result, WorkcellError, WorkcellRef, WorldRef};

pub(super) fn binding_ref(
    workcell_ref: &WorkcellRef,
    demand_ref: &DemandRef,
    logical_ref: &str,
    provider_ref: &ProviderRef,
    material_ref: &str,
) -> Result<BindingRef> {
    let value = fingerprint(&[
        workcell_ref.as_str(),
        demand_ref.as_str(),
        logical_ref,
        provider_ref.as_str(),
        material_ref,
    ]);
    BindingRef::new(format!("binding:{value:016x}"))
        .map_err(|error| WorkcellError::OperationFailed(error.into()))
}

pub(super) fn world_ref(
    workcell_ref: &WorkcellRef,
    demand_ref: &DemandRef,
    binding_refs: impl Iterator<Item = String>,
) -> Result<WorldRef> {
    let mut values = vec![workcell_ref.as_str().to_owned(), demand_ref.as_str().to_owned()];
    values.extend(binding_refs);
    values.sort();
    let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
    let value = fingerprint(&borrowed);
    WorldRef::new(format!("world:{value:016x}"))
        .map_err(|error| WorkcellError::OperationFailed(error.into()))
}

fn fingerprint(parts: &[&str]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
