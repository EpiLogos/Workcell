use crate::{
    CollectionBundle, DesiredMaterialState, Discovery, ExecutionDemand, ExposureBundle,
    MaterialisationPlan, MaterialisedExecutionWorld, ObservationBundle, ReconciliationResult,
    ReleaseResult, Result, WorldRef,
};

/// Provider-neutral Workcell control-plane contract.
///
/// This is an in-process Rust domain seam. Transport, daemon and asynchronous
/// composition remain deliberately separate decisions.
pub trait WorkcellControlPlane {
    fn discover(&self) -> Result<Discovery>;
    fn plan(&self, demand: &ExecutionDemand) -> Result<MaterialisationPlan>;
    fn prepare(&mut self, demand: &ExecutionDemand) -> Result<MaterialisedExecutionWorld>;
    fn observe(&self, world: &WorldRef) -> Result<ObservationBundle>;
    fn expose(&self, world: &WorldRef) -> Result<ExposureBundle>;
    fn collect(&self, world: &WorldRef) -> Result<CollectionBundle>;
    fn release(&mut self, world: &WorldRef) -> Result<ReleaseResult>;
    fn reconcile(&mut self, desired: &[DesiredMaterialState]) -> Result<ReconciliationResult>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WorkcellError, WorkcellRef};

    struct ContractFixture;

    impl WorkcellControlPlane for ContractFixture {
        fn discover(&self) -> Result<Discovery> {
            Ok(Discovery {
                workcell_ref: WorkcellRef::new("workcell:fixture").unwrap(),
                health: crate::HealthState::Healthy,
                offers: vec![],
            })
        }

        fn plan(&self, _: &ExecutionDemand) -> Result<MaterialisationPlan> {
            Err(WorkcellError::Unsupported("planner arrives in F.03".into()))
        }

        fn prepare(&mut self, _: &ExecutionDemand) -> Result<MaterialisedExecutionWorld> {
            Err(WorkcellError::Unsupported(
                "providers arrive in F.04".into(),
            ))
        }

        fn observe(&self, _: &WorldRef) -> Result<ObservationBundle> {
            Err(WorkcellError::NotFound("fixture has no worlds".into()))
        }

        fn expose(&self, _: &WorldRef) -> Result<ExposureBundle> {
            Err(WorkcellError::NotFound("fixture has no worlds".into()))
        }

        fn collect(&self, _: &WorldRef) -> Result<CollectionBundle> {
            Err(WorkcellError::NotFound("fixture has no worlds".into()))
        }

        fn release(&mut self, _: &WorldRef) -> Result<ReleaseResult> {
            Err(WorkcellError::NotFound("fixture has no worlds".into()))
        }

        fn reconcile(&mut self, _: &[DesiredMaterialState]) -> Result<ReconciliationResult> {
            Ok(ReconciliationResult { deltas: vec![] })
        }
    }

    #[test]
    fn complete_control_plane_surface_is_implementable_without_factory_types() {
        let fixture = ContractFixture;
        let discovered = fixture.discover().unwrap();
        assert_eq!(discovered.workcell_ref.as_str(), "workcell:fixture");
    }
}
