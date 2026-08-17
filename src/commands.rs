use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct IssueCommentEvent {
    issue: Issue,
    comment: Comment,
    sender: Option<Sender>,
}

#[derive(Debug, Deserialize)]
struct Issue {
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Comment {
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Sender {
    login: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueCommentCommand {
    pub login: String,
    pub full: bool,
}

pub fn parse_issue_comment(event_path: &str) -> anyhow::Result<Option<IssueCommentCommand>> {
    let raw = std::fs::read_to_string(event_path).context("failed to read GitHub event")?;
    let event: IssueCommentEvent = serde_json::from_str(&raw).context("invalid GitHub event")?;
    if event.issue.pull_request.is_none() {
        return Ok(None);
    }
    let Some(sender) = event.sender else {
        return Ok(None);
    };
    let body = event.comment.body.as_deref().unwrap_or_default().trim();
    match body {
        "/cururu review" => Ok(Some(IssueCommentCommand {
            login: sender.login,
            full: false,
        })),
        "/cururu review --full" => Ok(Some(IssueCommentCommand {
            login: sender.login,
            full: true,
        })),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn accepts_only_supported_pr_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        fs::write(
            &path,
            r#"{"issue":{"pull_request":{}},"comment":{"body":"/cururu review"},"sender":{"login":"alice"}}"#,
        )
        .unwrap();
        assert!(
            parse_issue_comment(path.to_str().unwrap())
                .unwrap()
                .is_some()
        );

        fs::write(
            &path,
            r#"{"issue":{"pull_request":{}},"comment":{"body":"/cururu review; rm -rf /"},"sender":{"login":"alice"}}"#,
        )
        .unwrap();
        assert!(
            parse_issue_comment(path.to_str().unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn ignores_regular_issue_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        fs::write(&path, r#"{"issue":{},"comment":{"body":"/cururu review"}}"#).unwrap();
        assert!(
            parse_issue_comment(path.to_str().unwrap())
                .unwrap()
                .is_none()
        );
    }
}
