use anyhow::Context;
use std::{path::Path, process::Command};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryIdentity {
    pub host: Option<String>,
    pub path: String,
}

impl RepositoryIdentity {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let value = value.trim();
        anyhow::ensure!(!value.is_empty(), "repository identity is empty");

        let (host, path) = Url::parse(value).map_or_else(
            |_| {
                if let Some((authority, path)) = value.split_once(':')
                    && (authority.contains('@') || !authority.contains('/'))
                {
                    let host = authority
                        .rsplit('@')
                        .next()
                        .filter(|host| !host.is_empty())
                        .map(str::to_ascii_lowercase);
                    (host, path.to_string())
                } else {
                    (None, value.to_string())
                }
            },
            |url| {
                let host = url.host_str().map(str::to_ascii_lowercase);
                (host, url.path().to_string())
            },
        );

        let mut path = path.trim_matches('/').to_string();
        if let Some(without_git) = path.strip_suffix(".git") {
            path = without_git.to_string();
        }
        let components: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
        anyhow::ensure!(
            components.len() >= 2 && components.iter().all(|part| *part != "." && *part != ".."),
            "repository identity must include namespace and repository"
        );

        Ok(Self { host, path })
    }

    pub fn owner_and_repository(&self) -> anyhow::Result<(&str, &str)> {
        let mut parts = self.path.split('/');
        let owner = parts.next().unwrap_or_default();
        let repository = parts.next().unwrap_or_default();
        anyhow::ensure!(
            !owner.is_empty() && !repository.is_empty() && parts.next().is_none(),
            "the configured SCM adapter requires a two-part repository path"
        );
        Ok((owner, repository))
    }

    pub fn from_origin_remote() -> anyhow::Result<Option<Self>> {
        let cwd = std::env::current_dir().context("failed to determine current directory")?;
        Self::from_origin_remote_at(&cwd)
    }

    fn from_origin_remote_at(directory: &Path) -> anyhow::Result<Option<Self>> {
        let output = Command::new("git")
            .current_dir(directory)
            .args(["remote", "get-url", "origin"])
            .output()
            .context("could not inspect the current Git checkout")?;
        if !output.status.success() {
            return Ok(None);
        }
        let remote =
            String::from_utf8(output.stdout).context("the origin remote URL is not valid UTF-8")?;
        Self::parse(&remote).map(Some)
    }
}

pub fn change_request_number_from_host_cli(provider: &str) -> Option<u64> {
    if !provider.eq_ignore_ascii_case("github") {
        return None;
    }
    let output = Command::new("gh")
        .args(["pr", "view", "--json", "number", "--jq", ".number"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    parse_change_request_number(&text)
}

fn parse_change_request_number(text: &str) -> Option<u64> {
    text.trim().parse().ok().or_else(|| {
        serde_json::from_str::<serde_json::Value>(text)
            .ok()?
            .get("number")?
            .as_u64()
    })
}

pub fn github_host_urls(identity: &RepositoryIdentity) -> (String, String) {
    let server_url = std::env::var("CURURU_SCM_SERVER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("GITHUB_SERVER_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            identity
                .host
                .as_deref()
                .map(|host| format!("https://{host}"))
        })
        .unwrap_or_else(|| "https://github.com".to_string());
    let api_url = std::env::var("CURURU_SCM_API_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("GITHUB_API_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| {
            if server_url == "https://github.com" {
                "https://api.github.com".to_string()
            } else {
                format!("{}/api/v3", server_url.trim_end_matches('/'))
            }
        });
    (api_url, server_url)
}

pub fn github_identity_from_environment() -> anyhow::Result<Option<RepositoryIdentity>> {
    if let Ok(value) = std::env::var("CURURU_REPOSITORY")
        && !value.trim().is_empty()
    {
        return RepositoryIdentity::parse(&value).map(Some);
    }
    if let Ok(value) = std::env::var("GITHUB_REPOSITORY")
        && !value.trim().is_empty()
    {
        let mut identity = RepositoryIdentity::parse(&value)?;
        if let Ok(server) = std::env::var("GITHUB_SERVER_URL")
            && let Ok(url) = Url::parse(&server)
        {
            identity.host = url.host_str().map(str::to_ascii_lowercase);
        }
        return Ok(Some(identity));
    }
    RepositoryIdentity::from_origin_remote()
}

pub fn validate_github_adapter(
    identity: &RepositoryIdentity,
    provider: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        provider.eq_ignore_ascii_case("github"),
        "the configured source-control adapter is not available in this build"
    );

    let has_custom_host = [
        "CURURU_SCM_SERVER_URL",
        "CURURU_SCM_API_URL",
        "GITHUB_SERVER_URL",
        "GITHUB_API_URL",
    ]
    .iter()
    .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()));
    anyhow::ensure!(
        identity
            .host
            .as_deref()
            .is_none_or(|host| host == "github.com" || has_custom_host),
        "the detected remote host has no configured source-control adapter"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_ssh_and_bare_repository_identities() {
        assert_eq!(
            RepositoryIdentity::parse("https://github.example/acme/service.git").unwrap(),
            RepositoryIdentity {
                host: Some("github.example".into()),
                path: "acme/service".into(),
            }
        );
        assert_eq!(
            RepositoryIdentity::parse("git@github.example:acme/service.git").unwrap(),
            RepositoryIdentity {
                host: Some("github.example".into()),
                path: "acme/service".into(),
            }
        );
        assert_eq!(
            RepositoryIdentity::parse("acme/service").unwrap(),
            RepositoryIdentity {
                host: None,
                path: "acme/service".into(),
            }
        );
    }

    #[test]
    fn rejects_repository_paths_that_cannot_map_to_owner_and_repository() {
        let identity = RepositoryIdentity::parse("group/subgroup/service").unwrap();
        assert!(identity.owner_and_repository().is_err());
        assert!(RepositoryIdentity::parse("../service").is_err());
    }

    #[test]
    fn reads_origin_remote_without_invoking_a_shell() {
        let directory = tempfile::tempdir().unwrap();
        let init = Command::new("git")
            .current_dir(directory.path())
            .args(["init", "--quiet"])
            .status()
            .unwrap();
        assert!(init.success());
        let remote = Command::new("git")
            .current_dir(directory.path())
            .args([
                "remote",
                "add",
                "origin",
                "git@example.test:team/project.git",
            ])
            .status()
            .unwrap();
        assert!(remote.success());

        assert_eq!(
            RepositoryIdentity::from_origin_remote_at(directory.path()).unwrap(),
            Some(RepositoryIdentity {
                host: Some("example.test".into()),
                path: "team/project".into(),
            })
        );
    }

    #[test]
    fn parses_host_cli_change_request_number_output() {
        assert_eq!(parse_change_request_number("42\n"), Some(42));
        assert_eq!(parse_change_request_number("{\"number\":42}"), Some(42));
        assert_eq!(parse_change_request_number("not a number"), None);
    }
}
