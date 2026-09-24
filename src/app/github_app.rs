use anyhow::Context;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct GitHubAppAuth {
    app_id: String,
    private_key: Vec<u8>,
    api_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct AppJwtClaims<'a> {
    iat: u64,
    exp: u64,
    iss: &'a str,
}

#[derive(Deserialize)]
struct InstallationTokenResponse {
    token: String,
}

impl GitHubAppAuth {
    pub fn new(app_id: &str, private_key: &str, api_url: &str) -> anyhow::Result<Self> {
        let private_key = private_key.replace("\\n", "\n").into_bytes();
        EncodingKey::from_rsa_pem(&private_key)
            .context("GITHUB_APP_PRIVATE_KEY is not a valid RSA PEM key")?;
        Ok(Self {
            app_id: app_id.to_string(),
            private_key,
            api_url: api_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .user_agent("cururu-github-app")
                .build()?,
        })
    }

    pub async fn installation_token(&self, installation_id: u64) -> anyhow::Result<String> {
        let jwt = self.app_jwt()?;
        let url = format!(
            "{}/app/installations/{installation_id}/access_tokens",
            self.api_url
        );
        let response = self
            .client
            .post(url)
            .timeout(std::time::Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(jwt)
            .send()
            .await
            .context("failed to exchange GitHub App JWT for installation token")?;
        if response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::FORBIDDEN
        {
            anyhow::bail!(
                "GitHub App installation {installation_id} is unavailable or not accessible to this App"
            );
        }
        response
            .error_for_status()
            .context("GitHub App installation-token request failed")?
            .json::<InstallationTokenResponse>()
            .await
            .context("invalid GitHub installation-token response")
            .map(|response| response.token)
    }

    pub async fn installation_for_repository(
        &self,
        repository: &str,
    ) -> anyhow::Result<Option<u64>> {
        let (owner, repo) = repository
            .split_once('/')
            .context("shared base repository must be owner/repository")?;
        let jwt = self.app_jwt()?;
        let response = self
            .client
            .get(format!(
                "{}/repos/{owner}/{repo}/installation",
                self.api_url
            ))
            .timeout(std::time::Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(jwt)
            .send()
            .await
            .context("failed to resolve GitHub App installation for shared base")?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .context("GitHub App cannot read the shared base installation")?
            .json::<InstallationIdResponse>()
            .await
            .context("invalid GitHub App installation response")?;
        Ok(Some(response.id))
    }

    fn app_jwt(&self) -> anyhow::Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_secs();
        let claims = AppJwtClaims {
            iat: now.saturating_sub(60),
            exp: now + 8 * 60,
            iss: &self.app_id,
        };
        encode(
            &Header::new(Algorithm::RS256),
            &claims,
            &EncodingKey::from_rsa_pem(&self.private_key)
                .context("failed to load GitHub App signing key")?,
        )
        .context("failed to sign GitHub App JWT")
    }
}

#[derive(Deserialize)]
struct InstallationIdResponse {
    id: u64,
}
