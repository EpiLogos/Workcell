use crate::{
    Availability, ExecutionDemand, HealthState, OperationalOffer, StorageAccess, StorageRequirement,
    StorageSharing,
};

use super::{policy::PlanningPolicy, requirements::MatchRule, requirements::RequirementAtom};

pub(crate) struct Resolution<'a> {
    pub(crate) selected: Option<&'a OperationalOffer>,
    pub(crate) reason: String,
}

pub(crate) fn resolve<'a>(
    demand: &ExecutionDemand,
    offers: &'a [OperationalOffer],
    policy: &dyn PlanningPolicy,
    requirement: &RequirementAtom,
) -> Resolution<'a> {
    let mut candidates = Vec::new();
    let mut capacity_shortfall = false;
    let mut unavailable = false;
    let mut policy_reasons = Vec::new();

    for offer in offers {
        match offer_match(offer, &requirement.rule) {
            OfferMatch::Unsupported => continue,
            OfferMatch::CapacityShortfall => {
                capacity_shortfall = true;
                continue;
            }
            OfferMatch::Matched => {}
        }
        if offer.availability == Availability::Unavailable
            || offer.health == HealthState::Unavailable
        {
            unavailable = true;
            continue;
        }
        let assessment = policy.assess(demand, offer);
        if !assessment.allowed {
            policy_reasons.push(
                assessment
                    .explanation
                    .unwrap_or_else(|| "policy rejected offer".into()),
            );
            continue;
        }
        candidates.push((offer, assessment.preference));
    }

    candidates.sort_by(|(left, left_preference), (right, right_preference)| {
        right_preference
            .cmp(left_preference)
            .then_with(|| operational_rank(right).cmp(&operational_rank(left)))
            .then_with(|| left.offer_ref.as_str().cmp(right.offer_ref.as_str()))
    });

    if let Some((offer, _)) = candidates.first() {
        return Resolution {
            selected: Some(*offer),
            reason: String::new(),
        };
    }

    let reason = if !policy_reasons.is_empty() {
        format!(
            "policy rejected matching offers: {}",
            policy_reasons.join("; ")
        )
    } else if capacity_shortfall {
        "matching offers have insufficient capacity".into()
    } else if unavailable {
        "matching offers are unavailable".into()
    } else {
        "no offer supports this material requirement".into()
    };
    Resolution {
        selected: None,
        reason,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OfferMatch {
    Matched,
    CapacityShortfall,
    Unsupported,
}

fn offer_match(offer: &OperationalOffer, rule: &MatchRule) -> OfferMatch {
    match rule {
        MatchRule::Affordance(value) => list_match(&offer.affordances, value),
        MatchRule::Connection(value) => list_match(&offer.connections, value),
        MatchRule::Exposure(value) => list_match(&offer.exposures, value),
        MatchRule::Isolation(value) => list_match(&offer.isolation_trust, value),
        MatchRule::Capacity(requirement) => capacity_match(offer, requirement),
        MatchRule::Storage(requirement) => storage_match(offer, requirement),
    }
}

fn capacity_match(
    offer: &OperationalOffer,
    requirement: &crate::ResourceRequirement,
) -> OfferMatch {
    match offer.capacity.get(&requirement.key) {
        Some(capacity) => {
            if requirement.unit.is_some() && requirement.unit != capacity.unit {
                OfferMatch::Unsupported
            } else if requirement
                .minimum
                .is_some_and(|minimum| capacity.amount < minimum)
            {
                OfferMatch::CapacityShortfall
            } else {
                OfferMatch::Matched
            }
        }
        None => OfferMatch::Unsupported,
    }
}

fn storage_match(offer: &OperationalOffer, requirement: &StorageRequirement) -> OfferMatch {
    if offer.port != "storage" || !offer.affordances.iter().any(|value| value == "storage:attached") {
        return OfferMatch::Unsupported;
    }
    if requirement.access == StorageAccess::Writable
        && !offer
            .affordances
            .iter()
            .any(|value| value == "storage:writable")
    {
        return OfferMatch::Unsupported;
    }
    if requirement.sharing == StorageSharing::Shared
        && !offer
            .affordances
            .iter()
            .any(|value| value == "storage:shared")
    {
        return OfferMatch::Unsupported;
    }
    if let Some(minimum) = requirement.minimum_capacity {
        let Some(capacity) = offer.capacity.get("storage") else {
            return OfferMatch::Unsupported;
        };
        if requirement.unit.is_some() && requirement.unit != capacity.unit {
            return OfferMatch::Unsupported;
        }
        if capacity.amount < minimum {
            return OfferMatch::CapacityShortfall;
        }
    }
    OfferMatch::Matched
}

fn list_match(values: &[String], value: &str) -> OfferMatch {
    if values.iter().any(|candidate| candidate == value) {
        OfferMatch::Matched
    } else {
        OfferMatch::Unsupported
    }
}

fn operational_rank(offer: &OperationalOffer) -> u8 {
    let availability = match offer.availability {
        Availability::Available => 2,
        Availability::Degraded => 1,
        Availability::Unavailable => 0,
    };
    let health = match offer.health {
        HealthState::Healthy => 2,
        HealthState::Degraded | HealthState::Unknown => 1,
        HealthState::Unavailable => 0,
    };
    availability + health
}
