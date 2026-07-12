use crate::rules::{Rec, Tier};
use std::collections::BTreeSet;

pub const GB: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalError {
    Empty,
    NotWholeGigabytes,
    Zero,
    ExceedsUsedSpace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub id: String,
    pub bytes: i64,
    pub estimate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimPlan {
    pub goal_bytes: i64,
    pub items: Vec<PlanItem>,
    pub selected_bytes: i64,
    pub measured_bytes: i64,
    pub estimated_bytes: i64,
    pub shortfall_bytes: i64,
    pub caution_bytes: i64,
}

pub struct OutcomeTracker {
    goal_bytes: i64,
    planned_bytes: i64,
    planned_estimated_bytes: i64,
    item_ids: BTreeSet<String>,
    failed_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimOutcome {
    pub goal_bytes: i64,
    pub planned_bytes: i64,
    pub planned_estimated_bytes: i64,
    pub actual_freed_bytes: i64,
    pub pending_trash_bytes: i64,
    pub goal_shortfall_bytes: i64,
    pub attempted_items: usize,
    pub failed_items: usize,
}

impl OutcomeTracker {
    pub fn new(
        goal_bytes: i64,
        planned_bytes: i64,
        planned_estimated_bytes: i64,
        item_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            goal_bytes: goal_bytes.max(0),
            planned_bytes: planned_bytes.max(0),
            planned_estimated_bytes: planned_estimated_bytes.max(0),
            item_ids: item_ids.into_iter().collect(),
            failed_ids: BTreeSet::new(),
        }
    }

    pub fn record_result(&mut self, id: &str, ok: bool) {
        if !ok && self.item_ids.contains(id) {
            self.failed_ids.insert(id.to_string());
        }
    }

    pub fn finish(self, freed: i64, pending: i64) -> ReclaimOutcome {
        let actual_freed_bytes = freed.max(0);
        ReclaimOutcome {
            goal_bytes: self.goal_bytes,
            planned_bytes: self.planned_bytes,
            planned_estimated_bytes: self.planned_estimated_bytes,
            actual_freed_bytes,
            pending_trash_bytes: pending.max(0),
            goal_shortfall_bytes: self.goal_bytes.saturating_sub(actual_freed_bytes).max(0),
            attempted_items: self.item_ids.len(),
            failed_items: self.failed_ids.len(),
        }
    }
}

pub fn parse_goal_gb(input: &str, used_bytes: i64) -> Result<i64, GoalError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(GoalError::Empty);
    }
    let whole_gb: i64 = input.parse().map_err(|_| GoalError::NotWholeGigabytes)?;
    if whole_gb <= 0 {
        return Err(GoalError::Zero);
    }
    let bytes = whole_gb
        .checked_mul(GB)
        .ok_or(GoalError::ExceedsUsedSpace)?;
    if used_bytes <= 0 || bytes > used_bytes {
        return Err(GoalError::ExceedsUsedSpace);
    }
    Ok(bytes)
}

pub fn build_plan(recs: &[Rec], goal_bytes: i64) -> ReclaimPlan {
    let goal_bytes = goal_bytes.max(0);
    let caution_bytes = recs
        .iter()
        .filter(|rec| rec.tier == Tier::Caution)
        .map(|rec| rec.bytes.max(0))
        .sum();
    let mut candidates: Vec<PlanItem> = recs
        .iter()
        .filter(|rec| rec.tier == Tier::Safe && rec.bytes > 0)
        .map(|rec| PlanItem {
            id: rec.id.clone(),
            bytes: rec.bytes,
            estimate: rec.estimate,
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.estimate
            .cmp(&right.estimate)
            .then(right.bytes.cmp(&left.bytes))
            .then(left.id.cmp(&right.id))
    });

    let mut items = Vec::new();
    let mut selected_bytes = 0i64;
    for item in candidates {
        if selected_bytes >= goal_bytes {
            break;
        }
        selected_bytes = selected_bytes.saturating_add(item.bytes);
        items.push(item);
    }
    let measured_bytes = items
        .iter()
        .filter(|item| !item.estimate)
        .map(|item| item.bytes)
        .sum();
    let estimated_bytes = items
        .iter()
        .filter(|item| item.estimate)
        .map(|item| item.bytes)
        .sum();

    ReclaimPlan {
        goal_bytes,
        items,
        selected_bytes,
        measured_bytes,
        estimated_bytes,
        shortfall_bytes: goal_bytes.saturating_sub(selected_bytes).max(0),
        caution_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Action, Rec, Tier};
    use std::path::PathBuf;

    fn rec(id: &str, bytes: i64, tier: Tier, estimate: bool) -> Rec {
        Rec {
            id: id.into(),
            title: id.into(),
            path: PathBuf::from(format!("/fixture/{id}")),
            display: format!("/fixture/{id}"),
            bytes,
            tier,
            desc: "fixture",
            restore: "fixture",
            action: Action::Trash,
            command: None,
            allow_trash: true,
            allow_delete: true,
            note: String::new(),
            estimate,
        }
    }

    #[test]
    fn goal_parser_requires_bounded_whole_gigabytes() {
        assert_eq!(parse_goal_gb("", 100 * GB), Err(GoalError::Empty));
        assert_eq!(
            parse_goal_gb("2.5", 100 * GB),
            Err(GoalError::NotWholeGigabytes)
        );
        assert_eq!(parse_goal_gb("0", 100 * GB), Err(GoalError::Zero));
        assert_eq!(
            parse_goal_gb("101", 100 * GB),
            Err(GoalError::ExceedsUsedSpace)
        );
        assert_eq!(parse_goal_gb(" 20 ", 100 * GB), Ok(20 * GB));
    }

    #[test]
    fn measured_safe_items_win_before_estimates_and_caution() {
        let recs = vec![
            rec("estimated-large", 30 * GB, Tier::Safe, true),
            rec("measured-six", 6 * GB, Tier::Safe, false),
            rec("measured-five", 5 * GB, Tier::Safe, false),
            rec("caution-huge", 100 * GB, Tier::Caution, false),
        ];
        let plan = build_plan(&recs, 10 * GB);
        let ids: Vec<_> = plan.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, vec!["measured-six", "measured-five"]);
        assert_eq!(plan.measured_bytes, 11 * GB);
        assert_eq!(plan.estimated_bytes, 0);
        assert_eq!(plan.shortfall_bytes, 0);
        assert_eq!(plan.caution_bytes, 100 * GB);
    }

    #[test]
    fn estimate_is_used_only_when_measured_items_cannot_reach_goal() {
        let recs = vec![
            rec("measured", 9 * GB, Tier::Safe, false),
            rec("estimated", 4 * GB, Tier::Safe, true),
        ];
        let plan = build_plan(&recs, 10 * GB);
        assert_eq!(plan.selected_bytes, 13 * GB);
        assert_eq!(plan.measured_bytes, 9 * GB);
        assert_eq!(plan.estimated_bytes, 4 * GB);
        assert_eq!(plan.shortfall_bytes, 0);
    }

    #[test]
    fn shortfall_and_stable_identifier_tie_break_are_deterministic() {
        let recs = vec![
            rec("zeta", 3 * GB, Tier::Safe, false),
            rec("alpha", 3 * GB, Tier::Safe, false),
        ];
        let plan = build_plan(&recs, 10 * GB);
        let ids: Vec<_> = plan.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zeta"]);
        assert_eq!(plan.shortfall_bytes, 4 * GB);
    }

    #[test]
    fn zero_and_negative_findings_never_enter_the_plan() {
        let recs = vec![
            rec("zero", 0, Tier::Safe, false),
            rec("negative", -1, Tier::Safe, false),
        ];
        let plan = build_plan(&recs, 5 * GB);
        assert!(plan.items.is_empty());
        assert_eq!(plan.shortfall_bytes, 5 * GB);
    }

    #[test]
    fn outcome_separates_actual_free_space_from_pending_trash() {
        let mut tracker =
            OutcomeTracker::new(20 * GB, 24 * GB, 4 * GB, ["a".to_string(), "b".to_string()]);
        tracker.record_result("a", true);
        tracker.record_result("b", false);
        tracker.record_result("b", false);
        tracker.record_result("unrelated", false);
        let outcome = tracker.finish(8 * GB, 6 * GB);
        assert_eq!(outcome.goal_bytes, 20 * GB);
        assert_eq!(outcome.planned_bytes, 24 * GB);
        assert_eq!(outcome.planned_estimated_bytes, 4 * GB);
        assert_eq!(outcome.actual_freed_bytes, 8 * GB);
        assert_eq!(outcome.pending_trash_bytes, 6 * GB);
        assert_eq!(outcome.goal_shortfall_bytes, 12 * GB);
        assert_eq!(outcome.attempted_items, 2);
        assert_eq!(outcome.failed_items, 1);
    }

    #[test]
    fn outcome_clamps_negative_event_values_and_completed_goals() {
        let tracker = OutcomeTracker::new(5 * GB, 5 * GB, 0, ["a".to_string()]);
        let outcome = tracker.finish(-1, -1);
        assert_eq!(outcome.actual_freed_bytes, 0);
        assert_eq!(outcome.pending_trash_bytes, 0);
        assert_eq!(outcome.goal_shortfall_bytes, 5 * GB);

        let outcome = OutcomeTracker::new(5 * GB, 6 * GB, 0, ["a".to_string()]).finish(6 * GB, 0);
        assert_eq!(outcome.goal_shortfall_bytes, 0);
    }
}
