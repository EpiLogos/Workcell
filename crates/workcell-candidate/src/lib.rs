use epilogos_workcell_core::{ExecutionDemand, ExternalRef, Result, WorkcellError};

pub const CANDIDATE_SUBJECT_KEY: &str = "candidate";

/// A thin integration view over the ordinary provider-neutral
/// `ExecutionDemand` for repeatedly materialising an externally owned
/// Candidate.
///
/// This type does not define Candidate identity, revisions or equivalence. It
/// only ensures that the semantic owner supplied one stable opaque reference
/// and that the same reference is carried through Workcell materialisation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateMaterialisationDemand {
    candidate_ref: ExternalRef,
    demand: ExecutionDemand,
}

impl CandidateMaterialisationDemand {
    pub fn new(candidate_ref: ExternalRef, mut demand: ExecutionDemand) -> Result<Self> {
        if let Some(existing) = demand.subjects.get(CANDIDATE_SUBJECT_KEY) {
            if existing != &candidate_ref {
                return Err(WorkcellError::InvalidDemand(format!(
                    "ExecutionDemand already carries candidate `{existing}` but materialisation view was given `{candidate_ref}`"
                )));
            }
        } else {
            demand
                .subjects
                .insert(CANDIDATE_SUBJECT_KEY.into(), candidate_ref.clone());
        }
        demand.validate()?;
        Ok(Self {
            candidate_ref,
            demand,
        })
    }

    pub fn from_execution_demand(demand: ExecutionDemand) -> Result<Self> {
        let candidate_ref = demand
            .subjects
            .get(CANDIDATE_SUBJECT_KEY)
            .cloned()
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(
                    "Candidate materialisation view requires an external `candidate` subject"
                        .into(),
                )
            })?;
        Self::new(candidate_ref, demand)
    }

    pub fn candidate_ref(&self) -> &ExternalRef {
        &self.candidate_ref
    }

    pub fn execution_demand(&self) -> &ExecutionDemand {
        &self.demand
    }

    pub fn into_execution_demand(self) -> ExecutionDemand {
        self.demand
    }

    /// Produce another ordinary demand for rematerialising the same semantic
    /// Candidate. Material details can be changed by editing the returned
    /// provider-neutral demand; this wrapper never decides that those changes
    /// create a new Candidate revision.
    pub fn rematerialisation_demand(&self) -> ExecutionDemand {
        self.demand.clone()
    }
}
