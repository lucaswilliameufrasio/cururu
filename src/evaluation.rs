use crate::{agent::ReviewFinding, diff::ChangedFile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationMode {
    Observe,
    Filter,
}

impl EvaluationMode {
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "observe" => Some(Self::Observe),
            "filter" => Some(Self::Filter),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindingJudgment {
    pub defect_probability: f32,
    pub severity: Option<SeverityJudgment>,
    pub severity_confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeverityJudgment {
    Critical,
    High,
    Medium,
    Low,
    Ignore,
}

impl SeverityJudgment {
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Critical => Some("critical"),
            Self::High => Some("high"),
            Self::Medium => Some("medium"),
            Self::Low => Some("low"),
            Self::Ignore => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatedFinding {
    pub finding: ReviewFinding,
    pub judgment: FindingJudgment,
    pub suppressed: bool,
    #[serde(default)]
    pub published: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub model: String,
    pub findings: Vec<EvaluatedFinding>,
    #[serde(default)]
    pub omitted_findings: usize,
    pub usage: Option<EvaluationUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationUsage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
}

pub fn apply_judgments(
    findings: Vec<ReviewFinding>,
    judgments: Vec<FindingJudgment>,
    mode: EvaluationMode,
) -> anyhow::Result<(Vec<ReviewFinding>, EvaluationReport)> {
    anyhow::ensure!(
        findings.len() == judgments.len(),
        "evaluator returned a different number of judgments than findings"
    );

    let mut accepted = Vec::with_capacity(findings.len());
    let mut evaluated = Vec::with_capacity(findings.len());
    for (mut finding, judgment) in findings.into_iter().zip(judgments) {
        anyhow::ensure!(
            judgment.defect_probability.is_finite()
                && (0.0..=1.0).contains(&judgment.defect_probability),
            "evaluator returned an invalid defect probability"
        );
        anyhow::ensure!(
            judgment
                .severity_confidence
                .is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value)),
            "evaluator returned an invalid severity confidence"
        );
        if mode == EvaluationMode::Filter
            && let Some(name) = judgment.severity.and_then(SeverityJudgment::as_str)
        {
            finding.severity = name.into();
        }
        let suppressed = mode == EvaluationMode::Filter
            && (judgment.defect_probability < 0.5
                || judgment.severity == Some(SeverityJudgment::Ignore));
        if !suppressed {
            accepted.push(finding.clone());
        }
        evaluated.push(EvaluatedFinding {
            finding,
            judgment,
            suppressed,
            published: false,
        });
    }

    Ok((
        accepted,
        EvaluationReport {
            model: String::new(),
            findings: evaluated,
            omitted_findings: 0,
            usage: None,
        },
    ))
}

pub fn mark_published(report: &mut EvaluationReport, findings: &[ReviewFinding]) {
    for evaluated in &mut report.findings {
        evaluated.published = findings.iter().any(|published| {
            published.path == evaluated.finding.path
                && published.line == evaluated.finding.line
                && published.title == evaluated.finding.title
                && published.message == evaluated.finding.message
                && published.source == evaluated.finding.source
                && published.rule == evaluated.finding.rule
        });
    }
}

#[derive(Debug, Serialize)]
struct JevRequest<'a> {
    state: Vec<JevFindingState<'a>>,
    model: &'a str,
    questions: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct JevFindingState<'a> {
    id: usize,
    path: &'a str,
    line: Option<u32>,
    title: &'a str,
    claim: &'a str,
    proposed_fix: &'a str,
    reported_severity: &'a str,
    evidence: String,
}

#[derive(Debug, Deserialize)]
struct JevResponse {
    model: String,
    answers: std::collections::HashMap<String, JevAnswer>,
    #[serde(default)]
    usage: Option<EvaluationUsage>,
}

#[derive(Debug, Deserialize)]
struct JevAnswer {
    #[serde(rename = "type")]
    kind: String,
    noul: Option<f32>,
    choice: Option<String>,
    confidence: Option<f32>,
}

pub async fn evaluate_with_jev(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    findings: &[ReviewFinding],
    changed_files: &[ChangedFile],
) -> anyhow::Result<EvaluationReport> {
    evaluate_with_jev_at(
        client,
        "https://api.typesafe.ai/v1/systemone",
        api_key,
        model,
        findings,
        changed_files,
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn evaluate_with_jev_at(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    findings: &[ReviewFinding],
    changed_files: &[ChangedFile],
) -> anyhow::Result<EvaluationReport> {
    if findings.is_empty() {
        return Ok(EvaluationReport {
            model: model.into(),
            findings: Vec::new(),
            omitted_findings: 0,
            usage: None,
        });
    }
    anyhow::ensure!(
        !api_key.is_empty(),
        "TYPESAFE_API_KEY is required when Jev evaluation is enabled"
    );

    let evaluated_findings = &findings[..findings.len().min(MAX_EVALUATED_FINDINGS)];
    let state: Vec<_> = evaluated_findings
        .iter()
        .enumerate()
        .map(|(id, finding)| JevFindingState {
            id,
            path: &finding.path,
            line: finding.line,
            title: &finding.title,
            claim: &finding.message,
            proposed_fix: &finding.suggestion,
            reported_severity: &finding.severity,
            evidence: changed_files
                .iter()
                .find(|file| file.path == finding.path)
                .map(|file| {
                    relevant_hunk(file, finding.line)
                        .chars()
                        .take(8_000)
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    let mut questions = serde_json::Map::new();
    for id in 0..evaluated_findings.len() {
        questions.insert(format!("finding_{id}_is_defect"), serde_json::json!({
            "type": "noul",
            "instructions": {
                "question": format!("Is finding {id} likely to describe a real defect in the proposed code change? Evaluate its claim against the finding details and `state[{id}].evidence`; uncertainty must be reflected in a probability near 0.5."),
                "finding_id": id
            },
            "criteria": {
                "true": "The finding identifies a plausible correctness, security, reliability, or behavioral defect caused or exposed by the change.",
                "false": "The finding is unsupported, stylistic only, not actionable, or not a defect."
            }
        }));
        questions.insert(format!("finding_{id}_severity"), serde_json::json!({
            "type": "choice",
            "instructions": {
                "question": format!("What is the appropriate severity for finding {id}, if it is a real defect? Use the finding details and `state[{id}].evidence`; choose ignore when it is not a useful actionable defect."),
                "finding_id": id
            },
            "criteria": {
                "critical": "Catastrophic impact or broad data/security loss requiring immediate action.",
                "high": "Major user-facing or security impact; should block the change.",
                "medium": "Meaningful defect with bounded impact; should be fixed soon.",
                "low": "Minor, localized defect with limited impact.",
                "ignore": "Not actionable, unsupported, or not a real defect."
            }
        }));
    }
    let request = JevRequest {
        state,
        model,
        questions: questions.into(),
    };
    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&request)
        .send()
        .await
        .context("failed to contact TypeSafe evaluator")?
        .error_for_status()
        .context("TypeSafe evaluator rejected the request")?
        .json::<JevResponse>()
        .await
        .context("TypeSafe evaluator returned an invalid response")?;

    let mut evaluated = Vec::with_capacity(evaluated_findings.len());
    for (id, finding) in evaluated_findings.iter().enumerate() {
        let defect = response
            .answers
            .get(&format!("finding_{id}_is_defect"))
            .context("TypeSafe response omitted defect judgment")?;
        anyhow::ensure!(
            defect.kind == "noul",
            "TypeSafe returned an unexpected defect judgment type"
        );
        let severity = response
            .answers
            .get(&format!("finding_{id}_severity"))
            .context("TypeSafe response omitted severity judgment")?;
        anyhow::ensure!(
            severity.kind == "choice",
            "TypeSafe returned an unexpected severity judgment type"
        );
        let defect_probability = defect
            .noul
            .context("TypeSafe defect judgment omitted probability")?;
        let selected = severity
            .choice
            .as_deref()
            .context("TypeSafe severity judgment omitted choice")?;
        let severity = match selected {
            "critical" => SeverityJudgment::Critical,
            "high" => SeverityJudgment::High,
            "medium" => SeverityJudgment::Medium,
            "low" => SeverityJudgment::Low,
            "ignore" => SeverityJudgment::Ignore,
            _ => anyhow::bail!("TypeSafe returned an unknown severity judgment"),
        };
        evaluated.push(EvaluatedFinding {
            finding: finding.clone(),
            judgment: FindingJudgment {
                defect_probability,
                severity: Some(severity),
                severity_confidence: response
                    .answers
                    .get(&format!("finding_{id}_severity"))
                    .and_then(|answer| answer.confidence),
            },
            suppressed: false,
            published: false,
        });
    }
    Ok(EvaluationReport {
        model: response.model,
        findings: evaluated,
        omitted_findings: findings.len() - evaluated_findings.len(),
        usage: response.usage,
    })
}

fn relevant_hunk(file: &ChangedFile, line: Option<u32>) -> String {
    let Some(line) = line else {
        return file.patch.chars().take(8_000).collect();
    };
    let lines: Vec<&str> = file.patch.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, text)| text.starts_with("@@ ").then_some(index))
        .collect();
    for (offset, start) in starts.iter().enumerate() {
        let header = lines[*start];
        let new_range = header
            .split_whitespace()
            .find(|part| part.starts_with('+'))
            .and_then(|part| part.strip_prefix('+'));
        let Some((range_start, range_len)) = new_range.and_then(|range| {
            let mut values = range.split(',');
            let start = values.next()?.parse::<u32>().ok()?;
            let count = values.next().unwrap_or("1").parse::<u32>().ok()?;
            Some((start, count))
        }) else {
            continue;
        };
        if (range_start..range_start.saturating_add(range_len)).contains(&line) {
            let end = starts.get(offset + 1).copied().unwrap_or(lines.len());
            return lines[*start..end].join("\n");
        }
    }
    file.patch.chars().take(8_000).collect()
}

use anyhow::Context;

pub const MAX_EVALUATED_FINDINGS: usize = 100;

#[cfg(test)]
mod tests {
    use super::*;

    fn finding() -> ReviewFinding {
        ReviewFinding {
            severity: "high".into(),
            path: "src/payments.rs".into(),
            line: Some(11),
            title: "Possible duplicate charge".into(),
            message: "Retry can charge twice".into(),
            suggestion: "Use an idempotency key".into(),
            confidence: 0.9,
            suggested_change: None,
            source: None,
            rule: None,
        }
    }

    fn changed_file() -> ChangedFile {
        ChangedFile {
            path: "src/payments.rs".into(),
            patch: "diff --git a/src/payments.rs b/src/payments.rs\n@@ -10,2 +10,2 @@\n- charge()\n+ charge_twice()\n".into(),
            right_lines: vec![11],
        }
    }

    #[test]
    fn observe_keeps_finding_even_when_jev_says_ignore() {
        let (kept, report) = apply_judgments(
            vec![finding()],
            vec![FindingJudgment {
                defect_probability: 0.1,
                severity: Some(SeverityJudgment::Ignore),
                severity_confidence: Some(0.9),
            }],
            EvaluationMode::Observe,
        )
        .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].severity, "high");
        assert!(!report.findings[0].suppressed);
    }

    #[test]
    fn filter_suppresses_but_keeps_the_judgment_auditable() {
        let (kept, report) = apply_judgments(
            vec![finding()],
            vec![FindingJudgment {
                defect_probability: 0.1,
                severity: Some(SeverityJudgment::Ignore),
                severity_confidence: Some(0.9),
            }],
            EvaluationMode::Filter,
        )
        .unwrap();
        assert!(kept.is_empty());
        assert!(report.findings[0].suppressed);
        assert_eq!(report.findings[0].finding.path, "src/payments.rs");
    }

    #[test]
    fn filter_applies_selected_severity_to_kept_finding() {
        let (kept, _) = apply_judgments(
            vec![finding()],
            vec![FindingJudgment {
                defect_probability: 0.95,
                severity: Some(SeverityJudgment::Medium),
                severity_confidence: Some(0.8),
            }],
            EvaluationMode::Filter,
        )
        .unwrap();
        assert_eq!(kept[0].severity, "medium");
    }

    #[test]
    fn rejects_invalid_or_misaligned_evaluator_results() {
        assert!(apply_judgments(vec![finding()], Vec::new(), EvaluationMode::Filter).is_err());
        assert!(
            apply_judgments(
                vec![finding()],
                vec![FindingJudgment {
                    defect_probability: f32::NAN,
                    severity: None,
                    severity_confidence: None,
                }],
                EvaluationMode::Filter
            )
            .is_err()
        );
    }

    #[test]
    fn audit_distinguishes_jev_suppression_from_cururu_policy_filtering() {
        let mut report = EvaluationReport {
            model: "jev-test".into(),
            findings: vec![EvaluatedFinding {
                finding: finding(),
                judgment: FindingJudgment {
                    defect_probability: 0.8,
                    severity: Some(SeverityJudgment::High),
                    severity_confidence: Some(0.9),
                },
                suppressed: false,
                published: false,
            }],
            omitted_findings: 0,
            usage: None,
        };
        mark_published(&mut report, &[]);
        assert!(!report.findings[0].suppressed);
        assert!(!report.findings[0].published);
        mark_published(&mut report, &[finding()]);
        assert!(report.findings[0].published);
    }

    #[tokio::test]
    async fn jev_request_uses_typed_questions_and_returns_auditable_judgments() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{header, method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(header("authorization", "Bearer secret-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    "finding_0_is_defect": {"type":"noul", "noul":0.97},
                    "finding_0_severity": {"type":"choice", "choice":"medium", "probabilities":{"critical":0.0,"high":0.1,"medium":0.85,"low":0.05,"ignore":0.0}, "confidence":0.81}
                },
                "usage": {"input_tokens":100,"output_tokens":20}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let report = evaluate_with_jev_at(
            &client,
            &format!("{}/v1/systemone", server.uri()),
            "secret-test",
            "jev-latest",
            &[finding()],
            &[changed_file()],
        )
        .await
        .unwrap();
        assert_eq!(report.model, "jev-1.13.0");
        assert!((report.findings[0].judgment.defect_probability - 0.97).abs() < f32::EPSILON);
        assert_eq!(
            report.findings[0].judgment.severity,
            Some(SeverityJudgment::Medium)
        );
        assert_eq!(report.findings[0].judgment.severity_confidence, Some(0.81));
        assert_eq!(report.usage.as_ref().unwrap().input_tokens, Some(100));
        assert!(!report.findings[0].suppressed);

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["state"][0]["path"], "src/payments.rs");
        assert_eq!(
            body["state"][0]["evidence"],
            "@@ -10,2 +10,2 @@\n- charge()\n+ charge_twice()"
        );
        assert_eq!(body["questions"]["finding_0_is_defect"]["type"], "noul");
        assert_eq!(body["questions"]["finding_0_severity"]["type"], "choice");
    }

    #[tokio::test]
    async fn empty_findings_do_not_require_a_key_or_make_a_remote_request() {
        let report = evaluate_with_jev_at(
            &reqwest::Client::new(),
            "not a URL",
            "",
            "jev-latest",
            &[],
            &[],
        )
        .await
        .unwrap();
        assert!(report.findings.is_empty());
        assert_eq!(report.model, "jev-latest");
    }

    #[tokio::test]
    async fn missing_typed_answer_fails_instead_of_treating_the_finding_as_approved() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {},
                "usage": {"input_tokens": 10, "output_tokens": 0}
            })))
            .mount(&server)
            .await;

        let error = evaluate_with_jev_at(
            &reqwest::Client::new(),
            &format!("{}/v1/systemone", server.uri()),
            "secret-test",
            "jev-latest",
            &[finding()],
            &[changed_file()],
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("omitted defect judgment"));
    }

    #[tokio::test]
    async fn request_caps_findings_and_reports_the_unevaluated_remainder() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let mut answers = serde_json::Map::new();
        for id in 0..MAX_EVALUATED_FINDINGS {
            answers.insert(
                format!("finding_{id}_is_defect"),
                serde_json::json!({"type":"noul", "noul":0.9}),
            );
            answers.insert(
                format!("finding_{id}_severity"),
                serde_json::json!({"type":"choice", "choice":"low", "confidence":0.8}),
            );
        }
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model":"jev-test", "answers":answers
            })))
            .mount(&server)
            .await;

        let findings = vec![finding(); MAX_EVALUATED_FINDINGS + 7];
        let report = evaluate_with_jev_at(
            &reqwest::Client::new(),
            &format!("{}/v1/systemone", server.uri()),
            "secret-test",
            "jev-latest",
            &findings,
            &[changed_file()],
        )
        .await
        .unwrap();

        assert_eq!(report.findings.len(), MAX_EVALUATED_FINDINGS);
        assert_eq!(report.omitted_findings, 7);
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body["state"].as_array().unwrap().len(),
            MAX_EVALUATED_FINDINGS
        );
        assert_eq!(
            body["questions"].as_object().unwrap().len(),
            MAX_EVALUATED_FINDINGS * 2
        );
    }

    #[tokio::test]
    async fn evaluator_http_errors_do_not_leak_response_body() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(429).set_body_string("private service detail"))
            .mount(&server)
            .await;

        let error = evaluate_with_jev_at(
            &reqwest::Client::new(),
            &format!("{}/v1/systemone", server.uri()),
            "secret-test",
            "jev-latest",
            &[finding()],
            &[changed_file()],
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("rejected the request"));
        assert!(!error.to_string().contains("private service detail"));
        assert!(!error.to_string().contains("secret-test"));
    }
}
