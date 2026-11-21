// SPDX-License-Identifier: Apache-2.0
//! Conservative, runtime-independent selection between exact and candidate plans.

/// Native physical granules, called marks here to distinguish them from rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEstimate {
    /// Bounded MergeTree reader Description; this is not a snapshot guarantee.
    pub source: String,
    pub selected_marks: u64,
    pub total_marks: u64,
}

/// A performance decision only. Both alternatives must retain the exact predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateDecision {
    Candidate,
    MissingEstimate,
    InvalidEstimate,
    DifferentPopulation,
    EmptyBaseline,
    InsufficientReduction,
}
impl CandidateDecision {
    pub fn use_candidate(self) -> bool {
        self == Self::Candidate
    }
    pub fn reason(self) -> &'static str {
        match self {
            Self::Candidate => "selected_marks_reduction",
            Self::MissingEstimate => "missing_estimate",
            Self::InvalidEstimate => "invalid_estimate",
            Self::DifferentPopulation => "incomparable_source_or_marks",
            Self::EmptyBaseline => "zero_baseline_marks",
            Self::InsufficientReduction => "insufficient_mark_reduction",
        }
    }
}

/// Select candidates only for at least 25% fewer marks in the same source.
/// Estimates affect execution cost, never the exact result predicate. Unknown,
/// empty, inconsistent or non-comparable estimates retain the exact-only plan.
pub fn choose_candidate(
    exact: Option<PlanEstimate>,
    candidate: Option<PlanEstimate>,
) -> CandidateDecision {
    let (Some(exact), Some(candidate)) = (exact, candidate) else {
        return CandidateDecision::MissingEstimate;
    };
    if !valid(&exact) || !valid(&candidate) {
        return CandidateDecision::InvalidEstimate;
    }
    if exact.source != candidate.source || exact.total_marks != candidate.total_marks {
        return CandidateDecision::DifferentPopulation;
    }
    if exact.selected_marks == 0 {
        return CandidateDecision::EmptyBaseline;
    }
    // Widening arithmetic avoids overflow even for caller-constructed u64 values.
    if u128::from(candidate.selected_marks) * 4 <= u128::from(exact.selected_marks) * 3 {
        CandidateDecision::Candidate
    } else {
        CandidateDecision::InsufficientReduction
    }
}

fn valid(estimate: &PlanEstimate) -> bool {
    valid_source(&estimate.source)
        && estimate.total_marks != 0
        && estimate.selected_marks <= estimate.total_marks
}
fn valid_source(source: &str) -> bool {
    !source.trim().is_empty() && source.len() <= 4096 && !source.chars().any(char::is_control)
}

const MAX_JSON_BYTES: usize = 256 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_CONTAINERS: usize = 2048;
const MAX_PLAN_NODES: usize = 24;
const MAX_INDEX_STAGES: usize = 64;

#[derive(serde::Deserialize)]
struct Document {
    #[serde(rename = "Plan")]
    plan: Plan,
}
#[derive(serde::Deserialize)]
struct Plan {
    #[serde(rename = "Node Type")]
    kind: NodeKind,
    #[serde(rename = "Description", default)]
    description: Option<String>,
    #[serde(rename = "Plans", default)]
    plans: Vec<Plan>,
    #[serde(rename = "Indexes", default)]
    indexes: Option<Vec<Index>>,
}
// An unknown plan operator is not evidence of the same physical read scope.
// Extend only with a native qualification. Multi-source and remote operators
// intentionally have no representation here.
#[derive(serde::Deserialize, PartialEq, Eq)]
enum NodeKind {
    ReadFromMergeTree,
    Expression,
    Filter,
    Aggregating,
    Sorting,
    Limit,
    Offset,
    Distinct,
}
#[derive(serde::Deserialize)]
struct Index {
    #[serde(rename = "Type")]
    _kind: IndexKind,
    #[serde(rename = "Initial Granules")]
    initial: u64,
    #[serde(rename = "Selected Granules")]
    selected: u64,
}
#[derive(serde::Deserialize)]
enum IndexKind {
    MinMax,
    Partition,
    PrimaryKey,
    Skip,
}

/// Read one native `EXPLAIN json=1, indexes=1` document (not an HTTP JSON wrapper).
///
/// Requires one supported unary plan ending in one MergeTree reader with a
/// consistent index chain. Returns the first stage's initial marks and the last
/// stage's selected marks, never a sum of overlapping stages. Missing indexes,
/// other readers/operators, duplicate relevant fields and malformed counts
/// yield no estimate. Unknown auxiliary fields are ignored.
///
/// Resource bounds: 256 KiB UTF-8 input, 64 JSON nesting levels, 2048 JSON
/// containers, 24 plan nodes, 64 index stages and a 4096-byte source description.
/// Parsing does not execute SQL and cannot remove rows from an exact query.
pub fn native_plan_estimate(json: &str) -> Option<PlanEstimate> {
    if json.len() > MAX_JSON_BYTES || !bounded_json_shape(json) {
        return None;
    }
    // Typed fields reject duplicates instead of serde_json::Value's last-wins
    // behavior. The structural precheck also bounds ignored nested metadata.
    let documents: Vec<Document> = serde_json::from_str(json).ok()?;
    let [document] = documents.as_slice() else {
        return None;
    };
    let mut node = &document.plan;
    for _ in 0..MAX_PLAN_NODES {
        if node.kind == NodeKind::ReadFromMergeTree {
            if !node.plans.is_empty() {
                return None;
            }
            let source = node.description.as_deref()?;
            if !valid_source(source) {
                return None;
            }
            let indexes = node.indexes.as_ref()?;
            if indexes.is_empty() || indexes.len() > MAX_INDEX_STAGES {
                return None;
            }
            let total_marks = indexes.first()?.initial;
            let mut selected_marks = total_marks;
            for index in indexes {
                if index.initial != selected_marks || index.selected > index.initial {
                    return None;
                }
                selected_marks = index.selected;
            }
            let estimate = PlanEstimate {
                source: source.to_owned(),
                selected_marks,
                total_marks,
            };
            return valid(&estimate).then_some(estimate);
        }
        if node.indexes.is_some() {
            return None;
        }
        let [child] = node.plans.as_slice() else {
            return None;
        };
        node = child;
    }
    None
}

// This is only a structural budget precheck, not a replacement JSON parser.
// Strings and escapes prevent braces in descriptions from spending nesting.
fn bounded_json_shape(json: &str) -> bool {
    let (mut depth, mut containers) = (0usize, 0usize);
    let (mut quoted, mut escaped) = (false, false);
    for byte in json.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    containers += 1;
                    if depth > MAX_JSON_DEPTH || containers > MAX_JSON_CONTAINERS {
                        return false;
                    }
                }
                b']' | b'}' => {
                    let Some(next) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = next;
                }
                _ => {}
            }
        }
    }
    depth == 0 && !quoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn estimate(selected_marks: u64, total_marks: u64) -> Option<PlanEstimate> {
        Some(PlanEstimate {
            source: "fixture.events".into(),
            selected_marks,
            total_marks,
        })
    }
    fn plan(selected: u64, total: u64) -> serde_json::Value {
        json!([{"Plan":{"Node Type":"Expression","Plans":[{
            "Node Type":"ReadFromMergeTree", "Description":"fixture.events",
            "Indexes":[{"Type":"PrimaryKey","Initial Granules":total,"Selected Granules":selected}]
        }]}}])
    }

    #[test]
    fn selection_requires_material_comparable_reduction() -> TestResult {
        assert!(choose_candidate(estimate(128, 128), estimate(16, 128)).use_candidate());
        assert!(choose_candidate(estimate(100, 128), estimate(75, 128)).use_candidate());
        assert!(!choose_candidate(estimate(100, 128), estimate(76, 128)).use_candidate());
        assert!(!choose_candidate(estimate(8, 8), estimate(8, 8)).use_candidate());
        assert!(!choose_candidate(estimate(8, 8), estimate(10, 16)).use_candidate());
        assert_eq!(
            choose_candidate(None, estimate(1, 8)),
            CandidateDecision::MissingEstimate
        );
        assert_eq!(
            choose_candidate(estimate(0, 8), estimate(0, 8)),
            CandidateDecision::EmptyBaseline
        );
        assert_eq!(
            choose_candidate(estimate(9, 8), estimate(1, 8)),
            CandidateDecision::InvalidEstimate
        );
        assert_eq!(
            choose_candidate(estimate(8, 8), estimate(9, 8)),
            CandidateDecision::InvalidEstimate
        );
        assert_eq!(
            choose_candidate(estimate(0, 0), estimate(0, 0)),
            CandidateDecision::InvalidEstimate
        );
        assert_eq!(
            choose_candidate(estimate(8, 8), estimate(1, 16)),
            CandidateDecision::DifferentPopulation
        );
        assert!(
            choose_candidate(
                estimate(u64::MAX, u64::MAX),
                estimate(u64::MAX / 2, u64::MAX)
            )
            .use_candidate()
        );
        Ok(())
    }

    #[test]
    fn native_counts_are_last_stage_not_sum_of_indexes() -> TestResult {
        let mut value = plan(128, 128);
        value[0]["Plan"]["Plans"][0]["Indexes"] = json!([
            {"Type":"PrimaryKey", "Initial Granules":128, "Selected Granules":128},
            {"Type":"Skip", "Name":"ng", "Description":"text GRANULARITY 100000000",
            "Initial Granules":128, "Selected Granules":16}
        ]);
        assert_eq!(native_plan_estimate(&value.to_string()), estimate(16, 128));
        assert_eq!(
            native_plan_estimate(&plan(0, 128).to_string()),
            estimate(0, 128)
        );
        value[0]["Plan"]["Plans"][0]["Indexes"][1]["Initial Granules"] = json!(64);
        assert_eq!(native_plan_estimate(&value.to_string()), None);
        Ok(())
    }

    #[test]
    fn unknown_and_multiple_readers_never_select_candidate() -> TestResult {
        for value in [
            json!([]),
            json!([{"Plan":{"Node Type":"ReadFromMergeTree"}}]),
            json!([{"Plan":{"Node Type":"ReadFromMemory"}}]),
            json!([{"Plan":{"Node Type":"Unknown","Plans":[plan(1, 8)[0]["Plan"].clone()]}}]),
            json!([{"Plan":{"Node Type":"Union","Plans":[plan(1, 8)[0]["Plan"].clone(),plan(1, 8)[0]["Plan"].clone()]}}]),
            json!([plan(1, 8)[0].clone(), plan(1, 8)[0].clone()]),
        ] {
            assert_eq!(native_plan_estimate(&value.to_string()), None);
        }
        Ok(())
    }

    #[test]
    fn malformed_ambiguous_and_oversized_counts_widen() -> TestResult {
        for input in [
            "",
            "null",
            "{}",
            "[",
            "[] trailing",
            "\0",
            "\u{fffd}",
            r#"[{"Plan":{"Node Type":"ReadFromMergeTree","Description":"fixture.events","Indexes":[{"Type":"Skip","Initial Granules":8,"Selected Granules":1,"Selected Granules":0}]}}]"#,
            r#"[{"Plan":{"Node Type":"ReadFromMergeTree","Node Type":"ReadFromMergeTree","Description":"fixture.events","Indexes":[{"Type":"Skip","Initial Granules":8,"Selected Granules":1}]}}]"#,
        ] {
            assert_eq!(native_plan_estimate(input), None);
        }
        for invalid in [json!(-1), json!(1.2), json!("1"), json!(null), json!(129)] {
            let mut value = plan(1, 128);
            value[0]["Plan"]["Plans"][0]["Indexes"][0]["Selected Granules"] = invalid;
            assert_eq!(native_plan_estimate(&value.to_string()), None);
        }
        let mut value = plan(1, 128);
        value[0]["Plan"]["Plans"][0]["Indexes"][0]["Type"] = json!("UnknownIndex");
        assert_eq!(native_plan_estimate(&value.to_string()), None);
        assert_eq!(native_plan_estimate(&" ".repeat(262145)), None);
        let mut nested = plan(1, 8)[0]["Plan"].clone();
        for _ in 0..80 {
            nested = json!({"Node Type":"Expression","Plans":[nested]});
        }
        assert_eq!(
            native_plan_estimate(&json!([{"Plan":nested}]).to_string()),
            None
        );
        Ok(())
    }

    #[test]
    fn source_identity_and_string_boundaries_are_required() -> TestResult {
        let mut other = PlanEstimate {
            source: "other.events".into(),
            selected_marks: 1,
            total_marks: 128,
        };
        assert_eq!(
            choose_candidate(estimate(128, 128), Some(other.clone())),
            CandidateDecision::DifferentPopulation
        );
        for source in ["".to_owned(), " ".into(), "x\0y".into(), "x".repeat(4097)] {
            let mut value = plan(1, 128);
            value[0]["Plan"]["Plans"][0]["Description"] = json!(source);
            assert_eq!(native_plan_estimate(&value.to_string()), None);
            other.source = source;
            assert_eq!(
                choose_candidate(estimate(128, 128), Some(other.clone())),
                CandidateDecision::InvalidEstimate
            );
        }
        let mut value = plan(1, 128);
        value[0]["Plan"]["Plans"][0]["Description"] = serde_json::Value::Null;
        assert_eq!(native_plan_estimate(&value.to_string()), None);
        value = plan(1, 128);
        value[0]["Plan"]["Plans"][0]["Indexes"][0]["Condition"] = json!("\"\\[]{}".repeat(10000));
        assert_eq!(native_plan_estimate(&value.to_string()), estimate(1, 128));
        Ok(())
    }
}
