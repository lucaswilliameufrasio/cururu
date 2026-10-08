use crate::{agent::ReviewFinding, diff::ChangedFile};
pub use cururu_core::{
    EvaluatedFinding, EvaluationMode, EvaluationReport, EvaluationUsage, FindingJudgment,
    SeverityJudgment, apply_judgments, mark_published,
};
use serde::{Deserialize, Serialize};

const JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

#[async_trait::async_trait]
pub trait ReviewEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        findings: &[ReviewFinding],
        changed_files: &[ChangedFile],
    ) -> anyhow::Result<EvaluationReport>;
}

struct JevEvaluator {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
}

impl JevEvaluator {
    fn new(client: reqwest::Client, api_key: String, model: String) -> Self {
        Self {
            client,
            endpoint: JEV_ENDPOINT.into(),
            api_key,
            model,
        }
    }

    #[cfg(test)]
    const fn with_endpoint(
        client: reqwest::Client,
        endpoint: String,
        api_key: String,
        model: String,
    ) -> Self {
        Self {
            client,
            endpoint,
            api_key,
            model,
        }
    }
}

#[async_trait::async_trait]
impl ReviewEvaluator for JevEvaluator {
    async fn evaluate(
        &self,
        findings: &[ReviewFinding],
        changed_files: &[ChangedFile],
    ) -> anyhow::Result<EvaluationReport> {
        evaluate_with_jev_at(
            &self.client,
            &self.endpoint,
            &self.api_key,
            &self.model,
            findings,
            changed_files,
        )
        .await
    }
}

pub fn build_jev_evaluator(
    client: reqwest::Client,
    api_key: String,
    model: String,
) -> Box<dyn ReviewEvaluator> {
    Box::new(JevEvaluator::new(client, api_key, model))
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

    let mut state = Vec::new();
    let mut evidence_budget = MAX_TOTAL_EVIDENCE_CHARS;
    for (id, finding) in findings.iter().take(MAX_EVALUATED_FINDINGS).enumerate() {
        let evidence = changed_files
            .iter()
            .find(|file| file.path == finding.path)
            .map(|file| {
                relevant_hunk(file, finding.line)
                    .chars()
                    .take(MAX_EVIDENCE_CHARS_PER_FINDING.min(evidence_budget))
                    .collect::<String>()
            })
            .unwrap_or_default();
        evidence_budget -= evidence.chars().count();
        state.push(JevFindingState {
            id,
            path: &finding.path,
            line: finding.line,
            title: &finding.title,
            claim: &finding.message,
            proposed_fix: &finding.suggestion,
            reported_severity: &finding.severity,
            evidence,
        });
        if evidence_budget == 0 {
            break;
        }
    }
    let evaluated_findings = &findings[..state.len()];
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
pub const MAX_EVIDENCE_CHARS_PER_FINDING: usize = 8_000;
pub const MAX_TOTAL_EVIDENCE_CHARS: usize = 64_000;

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
        let evaluator: Box<dyn ReviewEvaluator> = Box::new(JevEvaluator::with_endpoint(
            client,
            format!("{}/v1/systemone", server.uri()),
            "secret-test".into(),
            "jev-latest".into(),
        ));
        let report = evaluator
            .evaluate(&[finding()], &[changed_file()])
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
    async fn request_caps_aggregate_evidence_and_reports_the_unevaluated_remainder() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let expected_count = MAX_TOTAL_EVIDENCE_CHARS / MAX_EVIDENCE_CHARS_PER_FINDING;
        let mut answers = serde_json::Map::new();
        for id in 0..expected_count {
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

        let findings = vec![finding(); expected_count + 3];
        let mut file = changed_file();
        file.patch = format!("{}{}", "x".repeat(MAX_TOTAL_EVIDENCE_CHARS + 1), "\n");
        let report = evaluate_with_jev_at(
            &reqwest::Client::new(),
            &format!("{}/v1/systemone", server.uri()),
            "secret-test",
            "jev-latest",
            &findings,
            &[file],
        )
        .await
        .unwrap();

        assert_eq!(report.findings.len(), expected_count);
        assert_eq!(report.omitted_findings, 3);
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        let state = body["state"].as_array().unwrap();
        assert_eq!(state.len(), expected_count);
        let evidence_chars: usize = state
            .iter()
            .map(|item| item["evidence"].as_str().unwrap().chars().count())
            .sum();
        assert_eq!(evidence_chars, MAX_TOTAL_EVIDENCE_CHARS);
        assert!(state.iter().all(|item| {
            item["evidence"].as_str().unwrap().chars().count() <= MAX_EVIDENCE_CHARS_PER_FINDING
        }));
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
