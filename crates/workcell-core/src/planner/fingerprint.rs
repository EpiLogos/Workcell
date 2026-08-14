use crate::{
    Degradation, ExecutionDemand, PlanOmission, PlanRef, PlannedBinding, Result, WorkcellError,
};

pub(crate) fn make_plan_ref(
    demand: &ExecutionDemand,
    bindings: &[PlannedBinding],
    degradations: &[Degradation],
    omissions: &[PlanOmission],
) -> Result<PlanRef> {
    let mut value = demand.demand_ref.as_str().to_owned();
    for binding in bindings {
        value.push_str(&binding.logical_ref);
        value.push_str(binding.offer_ref.as_str());
    }
    for item in degradations {
        value.push_str(&item.requirement);
    }
    for item in omissions {
        value.push_str(&item.requirement);
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    PlanRef::new(format!("plan:{}:{hash:016x}", demand.demand_ref))
        .map_err(|error| WorkcellError::OperationFailed(error.into()))
}
