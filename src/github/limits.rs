//! Bounded repository sampling for limit suggestions.

use super::GitHubClient;
use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PullSummary {
    number: u64,
    head: PullHead,
}

#[derive(Debug, Deserialize)]
struct PullHead {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct PullSize {
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    #[serde(default)]
    changed_files: u64,
}

/// Size and recorded Cururu usage of one recent pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullSample {
    pub number: u64,
    pub changed_lines: u64,
    pub changed_files: u64,
    /// Largest completion tokens recorded by a Cururu review of its head.
    pub completion_tokens: Option<u64>,
}

impl GitHubClient {
    /// Sample up to `limit` recently updated pull requests.
    pub async fn sample_pull_requests(&self, limit: usize) -> anyhow::Result<Vec<PullSample>> {
        let pulls: Vec<PullSummary> = self
            .api(
                reqwest::Method::GET,
                &format!(
                    "/pulls?state=all&sort=updated&direction=desc&per_page={}",
                    limit.clamp(1, 100)
                ),
            )
            .send()
            .await
            .context("failed to list pull requests")?
            .error_for_status()
            .context("failed to read pull requests")?
            .json()
            .await
            .context("invalid pull request list")?;
        let mut samples = Vec::new();
        for pull in pulls.into_iter().take(limit) {
            let size: PullSize = self
                .api(reqwest::Method::GET, &format!("/pulls/{}", pull.number))
                .send()
                .await
                .context("failed to read a pull request")?
                .error_for_status()
                .context("failed to read a pull request")?
                .json()
                .await
                .context("invalid pull request")?;
            let completion_tokens = self
                .list_candidate_check_runs(&pull.head.sha)
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|check| check.output.summary.as_deref())
                .flat_map(crate::review_usage::parse_markers)
                .filter(|record| record.known)
                .map(|record| record.completion)
                .max();
            samples.push(PullSample {
                number: pull.number,
                changed_lines: size.additions + size.deletions,
                changed_files: size.changed_files,
                completion_tokens,
            });
        }
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GitHubConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn samples_sizes_and_recorded_completion_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"number": 1, "head": {"sha": "h1"}},
                {"number": 2, "head": {"sha": "h2"}}
            ])))
            .mount(&server)
            .await;
        for (number, additions) in [(1, 100), (2, 10)] {
            Mock::given(method("GET"))
                .and(path(format!("/repos/owner/repo/pulls/{number}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "additions": additions, "deletions": 5, "changed_files": 3
                })))
                .mount(&server)
                .await;
        }
        let mut record = crate::review_usage::UsageRecord::new("r", "h1", None);
        record.known = true;
        record.completion = 2500;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/commits/h1/check-runs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "check_runs": [{"id": 1, "output": {"summary": record.marker()}}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/commits/h2/check-runs"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"check_runs": []})),
            )
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "t".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 0,
            api_url: server.uri(),
            server_url: server.uri(),
        })
        .unwrap();
        let samples = client.sample_pull_requests(20).await.unwrap();
        assert_eq!(
            samples,
            vec![
                PullSample {
                    number: 1,
                    changed_lines: 105,
                    changed_files: 3,
                    completion_tokens: Some(2500)
                },
                PullSample {
                    number: 2,
                    changed_lines: 15,
                    changed_files: 3,
                    completion_tokens: None
                },
            ]
        );
    }
}
