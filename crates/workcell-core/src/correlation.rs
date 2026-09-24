//! Projection correlation: carrying an AIKit git-projection verdict on a
//! Workcell material world, without Workcell ever performing or re-deriving git.
//!
//! Workcell owns material lifecycle, not git. AIKit — the repository / worktree /
//! branch owner — decides whether a checkout projects its canonical target
//! (`origin/main`); that decision is an *opaque input* here. This module carries
//! the verdict, validates only carrier integrity (a non-empty subject that names
//! a material-world subject, and a recognised source tag), and correlates it to
//! the checkout subject on a material world.
//!
//! It computes nothing about git: no ancestry, no cleanliness, no remote read,
//! no branch resolution. `projected`, `surfaced`, `target` and `applied` are all
//! *supplied by the caller* (from an AIKit `SuiteProjection`,
//! `aikit.worktree-projection/v1`) and carried exactly as supplied. The
//! correlation answers "is this material world's checkout projecting main?" by
//! *carrying AIKit's answer*, never by deriving one. This module spawns no
//! subprocess of any kind, so in particular it runs no git.

use crate::{ExternalRef, MaterialisedExecutionWorld, Result, WorkcellError, WorldRef};

/// The AIKit worktree-projection schema tag this carrier recognises. The payload
/// is opaque to Workcell; the tag is validated only so an unrelated document is
/// refused rather than silently attributed as a projection verdict.
pub const AIKIT_WORKTREE_PROJECTION_SOURCE: &str = "aikit.worktree-projection/v1";

/// Recognised projection-verdict source tags. A correlation whose `source` is
/// not one of these is refused: Workcell will not attribute an unknown document
/// as a projection verdict it can carry.
pub const RECOGNISED_PROJECTION_SOURCES: &[&str] = &[AIKIT_WORKTREE_PROJECTION_SOURCE];

/// Convention: the `subjects` key prefix under which a material world carries an
/// opaque git-checkout correlation anchor. The value (an [`ExternalRef`]) names
/// the checkout / worktree AIKit projected; Workcell preserves it verbatim and
/// never parses it for git meaning. Using a well-known prefix keeps the anchor
/// discoverable without teaching Workcell any repository vocabulary.
pub const CHECKOUT_SUBJECT_PREFIX: &str = "checkout:";

/// An AIKit worktree-projection verdict, carried opaquely and correlated to a
/// material-world checkout subject.
///
/// Every field is *supplied by the caller* from an AIKit `SuiteProjection`.
/// Workcell computes none of them: it neither runs git nor inspects ancestry,
/// cleanliness or branches. The carrier exists so a "this environment projects
/// main" reading can travel *through* Workcell — attributed to its author —
/// without Workcell owning or re-deriving the git facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionCorrelation {
    /// The material-world `subjects` key this verdict is correlated to (e.g.
    /// `checkout:workcell`). Must name an existing subject on the world.
    pub subject_key: String,
    /// The schema tag of the verdict's source. Must be a recognised projection
    /// source (see [`RECOGNISED_PROJECTION_SOURCES`]).
    pub source: String,
    /// The canonical target the checkout(s) were projected onto, verbatim (e.g.
    /// `origin/main`). Opaque — Workcell never resolves or parses it.
    pub target: String,
    /// AIKit performed fast-forwards (apply mode) versus a read-only observe.
    /// Carried verbatim from the source verdict.
    pub applied: bool,
    /// AIKit's whole-suite verdict: every projected checkout ends at the target.
    /// Supplied by the caller, never derived by Workcell.
    pub projected: bool,
    /// Repo keys AIKit surfaced as needing a human (drift, or a read/apply
    /// failure). Opaque keys carried verbatim; Workcell neither derives them nor
    /// validates them against any git state.
    pub surfaced: Vec<String>,
    /// AIKit's plain-language reading, carried verbatim for disclosure. Empty
    /// when the caller supplied none.
    pub summary: String,
}

impl ProjectionCorrelation {
    /// Validate *carrier integrity only* — never the git facts.
    ///
    /// Checks the subject key is non-empty, the source tag is recognised, the
    /// target is non-empty, and no surfaced key is blank. Whether
    /// `projected` / `surfaced` are *true of the world's git* is AIKit's concern
    /// and is trusted here, not re-checked: Workcell has no way to know and no
    /// business deciding.
    pub fn validate_shape(&self) -> Result<()> {
        if self.subject_key.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "projection correlation subject key must not be empty".into(),
            ));
        }
        if self.target.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "projection correlation target must not be empty".into(),
            ));
        }
        if !RECOGNISED_PROJECTION_SOURCES.contains(&self.source.as_str()) {
            return Err(WorkcellError::Unsupported(format!(
                "projection verdict source `{}` is not a recognised projection source",
                self.source
            )));
        }
        for key in &self.surfaced {
            if key.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "projection correlation surfaced repo key must not be empty".into(),
                ));
            }
        }
        Ok(())
    }
}

/// A projection verdict folded onto a material world's observation: AIKit's
/// opaque verdict, attributed to its author and correlated to the checkout
/// subject it annotates.
///
/// This is a *reading*, never persisted material state — it is not a
/// [`DesiredMaterialState`](crate::DesiredMaterialState) and reconciling it
/// performs no git. A reader sees the verdict, who authored it, and which
/// material subject it is about, and can trust that Workcell added nothing to
/// the git facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrelatedObservation {
    /// The material world the verdict is an observation on.
    pub world_ref: WorldRef,
    /// The material-world subject key the verdict is correlated to.
    pub subject_key: String,
    /// The opaque external ref that subject carries on the world, verbatim. This
    /// is the git checkout / worktree AIKit projected; Workcell never parses it.
    pub subject: ExternalRef,
    /// Who authored the verdict (the source schema tag, e.g.
    /// `aikit.worktree-projection/v1`). Attribution is explicit so a reader never
    /// mistakes a carried AIKit verdict for a Workcell-computed fact.
    pub attributed_to: String,
    /// The carried verdict, verbatim.
    pub correlation: ProjectionCorrelation,
}

/// Fold a supplied projection verdict into a material world's observation.
///
/// This is the whole of Workcell's part: validate carrier integrity, resolve the
/// checkout subject the verdict annotates (it must already be a subject of the
/// world), and return the verdict attributed to its author and correlated to
/// that subject. Workcell performs no git and re-derives nothing — `projected`,
/// `surfaced`, `target` and `applied` are carried exactly as supplied.
///
/// Refuses (`NotFound`) a correlation whose subject is not on the world, and
/// (`Unsupported` / `InvalidDemand`) one whose carrier shape is invalid. It never
/// refuses on the *content* of the verdict: a `projected: false` reading with
/// surfaced drift is a perfectly valid thing to carry.
pub fn correlate_projection(
    world: &MaterialisedExecutionWorld,
    correlation: ProjectionCorrelation,
) -> Result<CorrelatedObservation> {
    correlation.validate_shape()?;

    let subject = world
        .subjects
        .get(&correlation.subject_key)
        .ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "projection correlation subject `{}` is not a subject of world `{}`",
                correlation.subject_key, world.world_ref
            ))
        })?
        .clone();

    Ok(CorrelatedObservation {
        world_ref: world.world_ref.clone(),
        subject_key: correlation.subject_key.clone(),
        subject,
        attributed_to: correlation.source.clone(),
        correlation,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        BindingGraph, DemandRef, HealthState, RetentionExpectation, WorkcellRef, WorldRef,
    };

    fn world_with_subjects(subjects: BTreeMap<String, ExternalRef>) -> MaterialisedExecutionWorld {
        MaterialisedExecutionWorld {
            world_ref: WorldRef::new("world:dev-environment").unwrap(),
            workcell_ref: WorkcellRef::new("workcell:local").unwrap(),
            demand_ref: DemandRef::new("demand:dev-environment").unwrap(),
            subjects,
            binding_graph: BindingGraph::default(),
            planned_exposures: vec![],
            planned_constraints: vec![],
            plan_degradations: vec![],
            plan_omissions: vec![],
            persistence: None,
            retention: RetentionExpectation::Release,
            state: HealthState::Healthy,
            provenance: BTreeMap::new(),
        }
    }

    fn checkout_world() -> MaterialisedExecutionWorld {
        let mut subjects = BTreeMap::new();
        // The opaque anchor: the git checkout AIKit projected. Workcell stores it
        // verbatim and never parses it.
        subjects.insert(
            format!("{CHECKOUT_SUBJECT_PREFIX}workcell"),
            ExternalRef::new("dev-environment:/Users/dev/worktrees/env-1/workcell").unwrap(),
        );
        world_with_subjects(subjects)
    }

    #[test]
    fn a_projected_checkout_verdict_is_carried_and_attributed_to_aikit() {
        let world = checkout_world();
        let correlation = ProjectionCorrelation {
            subject_key: format!("{CHECKOUT_SUBJECT_PREFIX}workcell"),
            source: AIKIT_WORKTREE_PROJECTION_SOURCE.to_owned(),
            target: "origin/main".to_owned(),
            applied: true,
            projected: true,
            surfaced: vec![],
            summary: "1/1 checkouts projected onto origin/main (apply)".to_owned(),
        };

        let observation = correlate_projection(&world, correlation).unwrap();

        // Attributed to AIKit, not Workcell.
        assert_eq!(observation.attributed_to, AIKIT_WORKTREE_PROJECTION_SOURCE);
        // Correlated to the checkout subject, whose opaque ref is preserved verbatim.
        assert_eq!(
            observation.subject_key,
            format!("{CHECKOUT_SUBJECT_PREFIX}workcell")
        );
        assert_eq!(
            observation.subject.as_str(),
            "dev-environment:/Users/dev/worktrees/env-1/workcell"
        );
        // The verdict is carried exactly as supplied.
        assert!(observation.correlation.projected);
        assert_eq!(observation.correlation.target, "origin/main");
        assert!(observation.correlation.applied);
        assert!(observation.correlation.surfaced.is_empty());
        assert_eq!(observation.world_ref, world.world_ref);
    }

    #[test]
    fn a_surfaced_behind_verdict_is_carried_verbatim_without_reinterpretation() {
        let world = checkout_world();
        let correlation = ProjectionCorrelation {
            subject_key: format!("{CHECKOUT_SUBJECT_PREFIX}workcell"),
            source: AIKIT_WORKTREE_PROJECTION_SOURCE.to_owned(),
            target: "origin/main".to_owned(),
            applied: false,
            projected: false,
            surfaced: vec!["o-i".to_owned(), "factory".to_owned()],
            summary: "0/3 checkouts projected onto origin/main (observe)".to_owned(),
        };

        let observation = correlate_projection(&world, correlation).unwrap();

        assert!(!observation.correlation.projected);
        // The repo keys AIKit surfaced are carried verbatim — Workcell derives
        // nothing about which repos need attention.
        assert_eq!(observation.correlation.surfaced, vec!["o-i", "factory"]);
        assert!(!observation.correlation.applied);
        assert_eq!(observation.attributed_to, AIKIT_WORKTREE_PROJECTION_SOURCE);
    }

    #[test]
    fn a_verdict_for_a_subject_absent_from_the_world_is_refused() {
        let world = checkout_world();
        let correlation = ProjectionCorrelation {
            subject_key: format!("{CHECKOUT_SUBJECT_PREFIX}not-on-this-world"),
            source: AIKIT_WORKTREE_PROJECTION_SOURCE.to_owned(),
            target: "origin/main".to_owned(),
            applied: false,
            projected: true,
            surfaced: vec![],
            summary: String::new(),
        };

        assert!(matches!(
            correlate_projection(&world, correlation),
            Err(WorkcellError::NotFound(_))
        ));
    }

    #[test]
    fn a_verdict_from_an_unrecognised_source_is_refused() {
        let world = checkout_world();
        let correlation = ProjectionCorrelation {
            subject_key: format!("{CHECKOUT_SUBJECT_PREFIX}workcell"),
            source: "some.other-tool.projection/v9".to_owned(),
            target: "origin/main".to_owned(),
            applied: false,
            projected: true,
            surfaced: vec![],
            summary: String::new(),
        };

        assert!(matches!(
            correlate_projection(&world, correlation),
            Err(WorkcellError::Unsupported(_))
        ));
    }

    #[test]
    fn a_blank_subject_key_or_target_is_refused_as_a_shape_error() {
        let blank_subject = ProjectionCorrelation {
            subject_key: "   ".to_owned(),
            source: AIKIT_WORKTREE_PROJECTION_SOURCE.to_owned(),
            target: "origin/main".to_owned(),
            applied: false,
            projected: true,
            surfaced: vec![],
            summary: String::new(),
        };
        assert!(matches!(
            blank_subject.validate_shape(),
            Err(WorkcellError::InvalidDemand(_))
        ));

        let blank_target = ProjectionCorrelation {
            subject_key: format!("{CHECKOUT_SUBJECT_PREFIX}workcell"),
            source: AIKIT_WORKTREE_PROJECTION_SOURCE.to_owned(),
            target: String::new(),
            applied: false,
            projected: true,
            surfaced: vec![],
            summary: String::new(),
        };
        assert!(matches!(
            blank_target.validate_shape(),
            Err(WorkcellError::InvalidDemand(_))
        ));
    }

    /// The ownership guard, by inspection: the correlation path spawns no
    /// subprocess, so in particular it runs no git. The forbidden tokens are
    /// assembled from parts so this test's own source never trips it.
    #[test]
    fn the_correlation_path_spawns_no_subprocess_and_so_runs_no_git() {
        let source = include_str!("correlation.rs");
        let spawn_tokens = [
            ("Comm", "and::new"),
            ("std::proc", "ess"),
            ("process::Comm", "and"),
            (".spa", "wn("),
        ];
        for (left, right) in spawn_tokens {
            let token = format!("{left}{right}");
            assert!(
                !source.contains(&token),
                "correlation must carry AIKit's verdict, never execute anything: found `{token}`"
            );
        }
    }
}
