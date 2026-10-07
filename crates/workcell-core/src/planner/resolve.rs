use crate::{
    Availability, ExecutionDemand, HealthState, OperationalOffer, StorageAccess,
    StorageRequirement, StorageSharing,
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
        if offer.availability == Availability::Unavailable {
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
            // Execution coherence: when the demand declares an isolation
            // requirement, an execution-shaped atom prefers an offer that
            // also carries that isolation, so affordance and isolation do
            // not bind different providers and produce a world that cannot
            // be materialised. This is a *preference among matching offers*
            // — the isolation atom itself remains an independent required
            // match, and a weaker offer still resolves when nothing better
            // exists (the provider then refuses honestly at prepare).
            .then_with(|| {
                let demanded_isolation =
                    requirement.kind == "affordance" && demand.isolation_trust.is_some();
                if demanded_isolation {
                    let iso = demand.isolation_trust.as_ref().map(|value| value.as_str());
                    let left_matches = iso
                        .map(|iso| left.isolation_trust.iter().any(|item| item == iso))
                        .unwrap_or(false);
                    let right_matches = iso
                        .map(|iso| right.isolation_trust.iter().any(|item| item == iso))
                        .unwrap_or(false);
                    right_matches.cmp(&left_matches)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
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
            // Unit-normalised comparison when both sides speak known units:
            // a floor of `512 MiB` must match an offer advertising `bytes`,
            // and `500m` a `count` — string equality alone would make the
            // same physical amount look unsupported. Unknown units keep the
            // exact-string law (mismatch = unsupported, never a guess).
            if let (Some(need), Some(have)) = (
                normalise_resource(requirement.minimum, requirement.unit.as_deref()),
                normalise_resource(Some(capacity.amount), capacity.unit.as_deref()),
            ) {
                return if need > have {
                    OfferMatch::CapacityShortfall
                } else {
                    OfferMatch::Matched
                };
            }
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

/// Normalise a resource amount into comparable canonical units: memory to
/// bytes, CPU to millicpu. `None` for both amount and unit means "not
/// stated", which normalises to zero. An amount without a unit is ambiguous
/// across resource kinds, so it normalises only for CPU counts and raw
/// bytes-shaped quantities are left unmatched unless the unit names them.
fn normalise_resource(amount: Option<u64>, unit: Option<&str>) -> Option<u64> {
    let amount = amount?;
    match unit.map(str::to_ascii_lowercase).as_deref() {
        None => Some(amount),
        Some("b") | Some("bytes") => Some(amount),
        Some("kib") | Some("ki") => amount.checked_mul(1024),
        Some("mib") | Some("mi") => amount.checked_mul(1024_u64.pow(2)),
        Some("gib") | Some("gi") => amount.checked_mul(1024_u64.pow(3)),
        Some("tib") | Some("ti") => amount.checked_mul(1024_u64.pow(4)),
        Some("count") | Some("cpu") | Some("cpus") | Some("cores") => amount.checked_mul(1000),
        Some("m") => Some(amount),
        Some(_) => None,
    }
}

fn storage_match(offer: &OperationalOffer, requirement: &StorageRequirement) -> OfferMatch {
    // A named attachment must not accidentally select a different logical store.
    if offer
        .metadata
        .get("logical_ref")
        .is_some_and(|r| r != &requirement.logical_ref)
        || (requirement.access == StorageAccess::ReadOnly
            && offer
                .metadata
                .get("storage:read-only")
                .is_some_and(|v| v == "unsupported"))
        || (requirement.sharing == StorageSharing::Exclusive
            && offer
                .metadata
                .get("storage:exclusive")
                .is_some_and(|v| v == "unsupported"))
    {
        return OfferMatch::Unsupported;
    }
    if offer.port != "storage"
        || !offer
            .affordances
            .iter()
            .any(|value| value == "storage:attached")
    {
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
