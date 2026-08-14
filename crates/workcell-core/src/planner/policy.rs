use crate::{ExecutionDemand, OperationalOffer};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyAssessment {
    pub allowed: bool,
    pub preference: i64,
    pub explanation: Option<String>,
}

impl PolicyAssessment {
    pub fn allow(preference: i64) -> Self {
        Self {
            allowed: true,
            preference,
            explanation: None,
        }
    }

    pub fn reject(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            preference: 0,
            explanation: Some(reason.into()),
        }
    }
}

pub trait PlanningPolicy {
    fn assess(&self, demand: &ExecutionDemand, offer: &OperationalOffer) -> PolicyAssessment;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NeutralPlanningPolicy;

impl PlanningPolicy for NeutralPlanningPolicy {
    fn assess(&self, _: &ExecutionDemand, _: &OperationalOffer) -> PolicyAssessment {
        PolicyAssessment::allow(0)
    }
}
