//! Per-review and per-PR usage accounting.
//!
//! Each hosted review run records its usage in a hidden marker on its
//! `Cururu review` check. Totals are summed from those checks across the PR's
//! commits, so reviews whose candidate was never approved still count.

use crate::provider::ProviderUsage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MARKER_PREFIX: &str = "<!-- cururu:usage:v1 ";
const MARKER_SUFFIX: &str = " -->";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageRecord {
    pub run: String,
    pub head: String,
    /// Whether the provider reported token usage for every model call.
    pub known: bool,
    pub prompt: u64,
    pub completion: u64,
    pub total: u64,
    pub cached: u64,
    pub reasoning: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

impl UsageRecord {
    pub fn new(run: &str, head: &str, usage: Option<&ProviderUsage>) -> Self {
        Self {
            run: run.into(),
            head: head.into(),
            known: usage.is_some(),
            prompt: usage.map_or(0, |usage| u64::from(usage.prompt_tokens)),
            completion: usage.map_or(0, |usage| u64::from(usage.completion_tokens)),
            total: usage.map_or(0, |usage| u64::from(usage.total_tokens)),
            cached: usage.map_or(0, |usage| u64::from(usage.cached_tokens)),
            reasoning: usage.map_or(0, |usage| u64::from(usage.reasoning_tokens)),
            cost: usage
                .and_then(|usage| usage.cost)
                .filter(|cost| cost.is_finite() && *cost >= 0.0),
        }
    }

    pub fn marker(&self) -> String {
        format!(
            "{MARKER_PREFIX}{}{MARKER_SUFFIX}",
            serde_json::to_string(self).unwrap_or_default()
        )
    }
}

/// Parse every usage marker in a check summary. Malformed markers are ignored.
pub fn parse_markers(text: &str) -> Vec<UsageRecord> {
    let mut records = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARKER_PREFIX) {
        let after = &rest[start + MARKER_PREFIX.len()..];
        let Some(end) = after.find(MARKER_SUFFIX) else {
            break;
        };
        if let Ok(record) = serde_json::from_str::<UsageRecord>(&after[..end]) {
            records.push(record);
        }
        rest = &after[end + MARKER_SUFFIX.len()..];
    }
    records
}

/// Usage records found for a PR, and whether the search covered every commit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageHistory {
    pub records: Vec<UsageRecord>,
    pub complete: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrUsage {
    pub reviews: usize,
    pub reviews_with_usage: usize,
    pub reviews_with_cost: usize,
    pub prompt: u64,
    pub completion: u64,
    pub total: u64,
    pub cost: f64,
    /// False when older history could not be read; totals are then a floor.
    pub complete: bool,
}

/// Sum the PR's recorded runs plus the current one, counting each run once.
pub fn pr_total(history: &UsageHistory, current: &UsageRecord) -> PrUsage {
    let mut runs: BTreeMap<&str, &UsageRecord> = history
        .records
        .iter()
        .map(|record| (record.run.as_str(), record))
        .collect();
    runs.insert(current.run.as_str(), current);
    let mut total = PrUsage {
        complete: history.complete,
        ..PrUsage::default()
    };
    for record in runs.values() {
        total.reviews += 1;
        if record.known {
            total.reviews_with_usage += 1;
            total.prompt = total.prompt.saturating_add(record.prompt);
            total.completion = total.completion.saturating_add(record.completion);
            total.total = total.total.saturating_add(record.total);
        }
        if let Some(cost) = record.cost {
            total.reviews_with_cost += 1;
            total.cost += cost;
        }
    }
    total
}

/// A unique identifier for one review run.
pub fn new_run_id(head: &str) -> String {
    use sha2::{Digest, Sha256};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let digest = Sha256::digest(format!("{head}:{nanos}:{}", std::process::id()).as_bytes());
    format!("{digest:x}")[..24].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(prompt: u32, cost: Option<f64>) -> ProviderUsage {
        ProviderUsage {
            prompt_tokens: prompt,
            completion_tokens: 10,
            total_tokens: prompt + 10,
            cached_tokens: 0,
            reasoning_tokens: 0,
            cost,
        }
    }

    #[test]
    fn markers_round_trip_and_ignore_malformed_or_foreign_text() {
        let record = UsageRecord::new("run1", "head", Some(&usage(100, Some(0.5))));
        let text = format!(
            "instructions\n{}\n<!-- cururu:usage:v1 {{not json}} -->\n{}",
            record.marker(),
            UsageRecord::new("run2", "head", None).marker()
        );
        let parsed = parse_markers(&text);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], record);
        assert!(!parsed[1].known);
    }

    #[test]
    fn totals_count_each_run_once_and_track_missing_cost_and_usage() {
        let first = UsageRecord::new("a", "h1", Some(&usage(100, Some(0.25))));
        let unknown = UsageRecord::new("b", "h1", None);
        let current = UsageRecord::new("c", "h2", Some(&usage(50, None)));
        let history = UsageHistory {
            records: vec![first.clone(), first, unknown, current.clone()],
            complete: true,
        };
        let total = pr_total(&history, &current);
        assert_eq!(total.reviews, 3);
        assert_eq!(total.reviews_with_usage, 2);
        assert_eq!(total.reviews_with_cost, 1);
        assert_eq!(
            (total.prompt, total.completion, total.total),
            (150, 20, 170)
        );
        assert!((total.cost - 0.25).abs() < f64::EPSILON);
        assert!(total.complete);
        let first_review = pr_total(&UsageHistory::default(), &current);
        assert_eq!(first_review.reviews, 1);
        assert!(!first_review.complete);
    }

    #[test]
    fn invalid_costs_are_not_recorded() {
        assert_eq!(
            UsageRecord::new("r", "h", Some(&usage(1, Some(f64::NAN)))).cost,
            None
        );
        assert_eq!(
            UsageRecord::new("r", "h", Some(&usage(1, Some(-1.0)))).cost,
            None
        );
    }
}
