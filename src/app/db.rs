use anyhow::{Context, bail};
use sqlx::{AnyPool, any::AnyPoolOptions};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct QueuedDelivery {
    pub id: String,
    pub event: String,
    pub payload: String,
    pub attempts: i32,
}

/// A hosted publication candidate awaiting or past approval.
#[derive(Debug, Clone)]
pub struct StoredCandidate {
    pub pr_number: u64,
    pub digest: String,
    pub state: String,
    pub check_run_id: Option<u64>,
    pub candidate: String,
}

/// Pending candidates older than this can no longer be approved.
pub const CANDIDATE_APPROVAL_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Clone)]
pub struct DeliveryStore {
    pool: AnyPool,
    sqlite: bool,
}

impl DeliveryStore {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let sqlite = database_url.starts_with("sqlite:");
        if !sqlite
            && !database_url.starts_with("postgres://")
            && !database_url.starts_with("postgresql://")
        {
            bail!("CURURU_DATABASE_URL must be a sqlite: or postgres:// URL");
        }

        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context("failed to connect to Cururu event database")?;
        if sqlite {
            sqlx::query("PRAGMA journal_mode = WAL")
                .execute(&pool)
                .await
                .context("failed to enable SQLite WAL mode")?;
            sqlx::query("PRAGMA synchronous = FULL")
                .execute(&pool)
                .await
                .context("failed to set SQLite durability mode")?;
        }

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS github_deliveries (
                delivery_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                state TEXT NOT NULL,
                attempts BIGINT NOT NULL DEFAULT 0,
                available_at BIGINT NOT NULL,
                received_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .context("failed to initialize webhook delivery queue")?;
        sqlx::query("CREATE INDEX IF NOT EXISTS github_deliveries_ready ON github_deliveries(state, available_at, received_at)")
            .execute(&pool)
            .await
            .context("failed to index webhook delivery queue")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS github_mention_limits (
                repository TEXT NOT NULL,
                pr_number BIGINT NOT NULL,
                user_login TEXT NOT NULL,
                window_started_at BIGINT NOT NULL,
                request_count BIGINT NOT NULL,
                PRIMARY KEY (repository, pr_number, user_login)
            )",
        )
        .execute(&pool)
        .await
        .context("failed to initialize GitHub mention rate limits")?;

        initialize_candidates(&pool, sqlite).await?;

        let now = epoch_seconds();
        let recover_query = if sqlite {
            "UPDATE github_deliveries SET state = 'queued', updated_at = ? WHERE state = 'processing' AND updated_at < ?"
        } else {
            "UPDATE github_deliveries SET state = 'queued', updated_at = $1 WHERE state = 'processing' AND updated_at < $2"
        };
        sqlx::query(recover_query)
            .bind(now)
            .bind(now - 300)
            .execute(&pool)
            .await
            .context("failed to recover interrupted webhook deliveries")?;
        let prune_query = if sqlite {
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < ?"
        } else {
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < $1"
        };
        sqlx::query(prune_query)
            .bind(now - 7 * 24 * 60 * 60)
            .execute(&pool)
            .await
            .context("failed to prune old webhook deliveries")?;

        Ok(Self { pool, sqlite })
    }

    pub async fn enqueue(&self, id: &str, event: &str, payload: &str) -> anyhow::Result<bool> {
        let now = epoch_seconds();
        let query = self.sql(
            "INSERT INTO github_deliveries (delivery_id, event_type, payload, state, attempts, available_at, received_at, updated_at) VALUES (?, ?, ?, 'queued', 0, ?, ?, ?) ON CONFLICT(delivery_id) DO NOTHING",
            "INSERT INTO github_deliveries (delivery_id, event_type, payload, state, attempts, available_at, received_at, updated_at) VALUES ($1, $2, $3, 'queued', 0, $4, $5, $6) ON CONFLICT(delivery_id) DO NOTHING",
        );
        let result = sqlx::query(query)
            .bind(id)
            .bind(event)
            .bind(payload)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .context("failed to enqueue GitHub webhook")?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn claim_next(&self) -> anyhow::Result<Option<QueuedDelivery>> {
        let now = epoch_seconds();
        let query = self.sql(
            "SELECT delivery_id, event_type, payload, attempts FROM github_deliveries WHERE state = 'queued' AND available_at <= ? ORDER BY received_at LIMIT 1",
            "SELECT delivery_id, event_type, payload, attempts FROM github_deliveries WHERE state = 'queued' AND available_at <= $1 ORDER BY received_at LIMIT 1",
        );
        let row = sqlx::query(query)
            .bind(now)
            .fetch_optional(&self.pool)
            .await
            .context("failed to find queued webhook")?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id: String = sqlx::Row::try_get(&row, "delivery_id")?;
        let event: String = sqlx::Row::try_get(&row, "event_type")?;
        let payload: String = sqlx::Row::try_get(&row, "payload")?;
        let attempts: i64 = sqlx::Row::try_get(&row, "attempts")?;
        let query = self.sql(
            "UPDATE github_deliveries SET state = 'processing', attempts = attempts + 1, updated_at = ? WHERE delivery_id = ? AND state = 'queued'",
            "UPDATE github_deliveries SET state = 'processing', attempts = attempts + 1, updated_at = $1 WHERE delivery_id = $2 AND state = 'queued'",
        );
        let result = sqlx::query(query)
            .bind(now)
            .bind(&id)
            .execute(&self.pool)
            .await
            .context("failed to claim webhook delivery")?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(Some(QueuedDelivery {
            id,
            event,
            payload,
            attempts: i32::try_from(attempts + 1).unwrap_or(i32::MAX),
        }))
    }

    pub async fn finish(&self, id: &str) -> anyhow::Result<()> {
        let query = self.sql(
            "UPDATE github_deliveries SET state = 'done', updated_at = ? WHERE delivery_id = ?",
            "UPDATE github_deliveries SET state = 'done', updated_at = $1 WHERE delivery_id = $2",
        );
        sqlx::query(query)
            .bind(epoch_seconds())
            .bind(id)
            .execute(&self.pool)
            .await
            .context("failed to mark webhook complete")?;
        Ok(())
    }

    pub async fn retry(&self, delivery: &QueuedDelivery) -> anyhow::Result<()> {
        let failed = delivery.attempts >= 5;
        let backoff = i64::from(delivery.attempts.min(8)).pow(2) * 5;
        let query = self.sql(
            "UPDATE github_deliveries SET state = ?, available_at = ?, updated_at = ? WHERE delivery_id = ?",
            "UPDATE github_deliveries SET state = $1, available_at = $2, updated_at = $3 WHERE delivery_id = $4",
        );
        sqlx::query(query)
            .bind(if failed { "failed" } else { "queued" })
            .bind(epoch_seconds() + backoff)
            .bind(epoch_seconds())
            .bind(&delivery.id)
            .execute(&self.pool)
            .await
            .context("failed to schedule webhook retry")?;
        Ok(())
    }

    pub async fn prune_old(&self) -> anyhow::Result<u64> {
        let query = self.sql(
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < ?",
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < $1",
        );
        let result = sqlx::query(query)
            .bind(epoch_seconds() - 7 * 24 * 60 * 60)
            .execute(&self.pool)
            .await
            .context("failed to prune old webhook deliveries")?;
        let mention_prune_query = self.sql(
            "DELETE FROM github_mention_limits WHERE window_started_at < ?",
            "DELETE FROM github_mention_limits WHERE window_started_at < $1",
        );
        sqlx::query(mention_prune_query)
            .bind(epoch_seconds() - 30 * 24 * 60 * 60)
            .execute(&self.pool)
            .await
            .context("failed to prune old GitHub mention rate limits")?;
        sqlx::query(self.sql(
            "DELETE FROM hosted_candidates WHERE updated_at < ?",
            "DELETE FROM hosted_candidates WHERE updated_at < $1",
        ))
        .bind(epoch_seconds() - 30 * 24 * 60 * 60)
        .execute(&self.pool)
        .await
        .context("failed to prune old hosted candidates")?;
        Ok(result.rows_affected())
    }

    /// Store a new pending candidate and supersede earlier pending ones for
    /// the same PR. A candidate that was already claimed or published keeps
    /// its state. Returns whether the candidate is pending.
    pub async fn store_candidate(
        &self,
        repository: &str,
        pr_number: u64,
        digest: &str,
        head_sha: &str,
        candidate: &str,
    ) -> anyhow::Result<bool> {
        let now = epoch_seconds();
        let pr = i64::try_from(pr_number).unwrap_or(i64::MAX);
        let mut transaction = self.pool.begin().await?;
        sqlx::query(self.sql(
            "UPDATE hosted_candidates SET state = 'superseded', updated_at = ? WHERE repository = ? AND pr_number = ? AND state = 'pending' AND digest <> ?",
            "UPDATE hosted_candidates SET state = 'superseded', updated_at = $1 WHERE repository = $2 AND pr_number = $3 AND state = 'pending' AND digest <> $4",
        ))
        .bind(now)
        .bind(repository)
        .bind(pr)
        .bind(digest)
        .execute(&mut *transaction)
        .await
        .context("failed to supersede earlier candidates")?;
        sqlx::query(self.sql(
            "INSERT INTO hosted_candidates (repository, pr_number, digest, head_sha, candidate, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'pending', ?, ?) ON CONFLICT(repository, digest) DO UPDATE SET state = 'pending', check_run_id = NULL, created_at = excluded.created_at, updated_at = excluded.updated_at WHERE hosted_candidates.state IN ('superseded', 'invalidated')",
            "INSERT INTO hosted_candidates (repository, pr_number, digest, head_sha, candidate, state, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7) ON CONFLICT(repository, digest) DO UPDATE SET state = 'pending', check_run_id = NULL, created_at = excluded.created_at, updated_at = excluded.updated_at WHERE hosted_candidates.state IN ('superseded', 'invalidated')",
        ))
        .bind(repository)
        .bind(pr)
        .bind(digest)
        .bind(head_sha)
        .bind(candidate)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .context("failed to store hosted candidate")?;
        transaction.commit().await?;
        Ok(self
            .find_candidate(repository, digest)
            .await?
            .is_some_and(|stored| stored.state == "pending"))
    }

    pub async fn set_candidate_check_run(
        &self,
        repository: &str,
        digest: &str,
        check_run_id: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(self.sql(
            "UPDATE hosted_candidates SET check_run_id = ?, updated_at = ? WHERE repository = ? AND digest = ?",
            "UPDATE hosted_candidates SET check_run_id = $1, updated_at = $2 WHERE repository = $3 AND digest = $4",
        ))
        .bind(i64::try_from(check_run_id).unwrap_or(i64::MAX))
        .bind(epoch_seconds())
        .bind(repository)
        .bind(digest)
        .execute(&self.pool)
        .await
        .context("failed to record candidate check")?;
        Ok(())
    }

    pub async fn has_pending_candidate_for_head(
        &self,
        repository: &str,
        pr_number: u64,
        head_sha: &str,
    ) -> anyhow::Result<bool> {
        let row = sqlx::query(self.sql(
            "SELECT digest FROM hosted_candidates WHERE repository = ? AND pr_number = ? AND head_sha = ? AND state IN ('pending', 'publishing') AND created_at >= ? LIMIT 1",
            "SELECT digest FROM hosted_candidates WHERE repository = $1 AND pr_number = $2 AND head_sha = $3 AND state IN ('pending', 'publishing') AND created_at >= $4 LIMIT 1",
        ))
        .bind(repository)
        .bind(i64::try_from(pr_number).unwrap_or(i64::MAX))
        .bind(head_sha)
        .bind(epoch_seconds() - CANDIDATE_APPROVAL_TTL_SECS)
        .fetch_optional(&self.pool)
        .await
        .context("failed to look up pending candidates")?;
        Ok(row.is_some())
    }

    pub async fn find_candidate(
        &self,
        repository: &str,
        digest: &str,
    ) -> anyhow::Result<Option<StoredCandidate>> {
        let row = sqlx::query(self.sql(
            "SELECT pr_number, digest, state, check_run_id, candidate FROM hosted_candidates WHERE repository = ? AND digest = ?",
            "SELECT pr_number, digest, state, check_run_id, candidate FROM hosted_candidates WHERE repository = $1 AND digest = $2",
        ))
        .bind(repository)
        .bind(digest)
        .fetch_optional(&self.pool)
        .await
        .context("failed to read hosted candidate")?;
        row.map(|row| stored_candidate(&row)).transpose()
    }

    pub async fn find_candidate_by_check_run(
        &self,
        repository: &str,
        check_run_id: u64,
    ) -> anyhow::Result<Option<StoredCandidate>> {
        let row = sqlx::query(self.sql(
            "SELECT pr_number, digest, state, check_run_id, candidate FROM hosted_candidates WHERE repository = ? AND check_run_id = ?",
            "SELECT pr_number, digest, state, check_run_id, candidate FROM hosted_candidates WHERE repository = $1 AND check_run_id = $2",
        ))
        .bind(repository)
        .bind(i64::try_from(check_run_id).unwrap_or(i64::MAX))
        .fetch_optional(&self.pool)
        .await
        .context("failed to read hosted candidate")?;
        row.map(|row| stored_candidate(&row)).transpose()
    }

    /// Atomically move an unexpired pending candidate to `publishing`. Only one
    /// concurrent or duplicate approval can win.
    pub async fn claim_candidate(
        &self,
        repository: &str,
        digest: &str,
        approver: &str,
    ) -> anyhow::Result<bool> {
        self.claim_candidate_at(repository, digest, approver, epoch_seconds())
            .await
    }

    async fn claim_candidate_at(
        &self,
        repository: &str,
        digest: &str,
        approver: &str,
        now: i64,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(self.sql(
            "UPDATE hosted_candidates SET state = 'publishing', approved_by = ?, updated_at = ? WHERE repository = ? AND digest = ? AND state = 'pending' AND created_at >= ?",
            "UPDATE hosted_candidates SET state = 'publishing', approved_by = $1, updated_at = $2 WHERE repository = $3 AND digest = $4 AND state = 'pending' AND created_at >= $5",
        ))
        .bind(approver)
        .bind(now)
        .bind(repository)
        .bind(digest)
        .bind(now - CANDIDATE_APPROVAL_TTL_SECS)
        .execute(&self.pool)
        .await
        .context("failed to claim hosted candidate")?;
        Ok(result.rows_affected() == 1)
    }

    /// Record the outcome of a claimed candidate.
    pub async fn finish_candidate(
        &self,
        repository: &str,
        digest: &str,
        state: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(state, "published" | "invalidated" | "uncertain"),
            "invalid candidate outcome"
        );
        sqlx::query(self.sql(
            "UPDATE hosted_candidates SET state = ?, updated_at = ? WHERE repository = ? AND digest = ? AND state = 'publishing'",
            "UPDATE hosted_candidates SET state = $1, updated_at = $2 WHERE repository = $3 AND digest = $4 AND state = 'publishing'",
        ))
        .bind(state)
        .bind(epoch_seconds())
        .bind(repository)
        .bind(digest)
        .execute(&self.pool)
        .await
        .context("failed to record publication outcome")?;
        Ok(())
    }

    pub async fn allow_mention(
        &self,
        repository: &str,
        pr_number: u64,
        user_login: &str,
        window_secs: i64,
        max_requests: i64,
    ) -> anyhow::Result<bool> {
        self.allow_mention_at(
            repository,
            pr_number,
            user_login,
            epoch_seconds(),
            window_secs,
            max_requests,
        )
        .await
    }

    async fn allow_mention_at(
        &self,
        repository: &str,
        pr_number: u64,
        user_login: &str,
        now: i64,
        window_secs: i64,
        max_requests: i64,
    ) -> anyhow::Result<bool> {
        let cutoff = now.saturating_sub(window_secs);
        let query = self.sql(
            "INSERT INTO github_mention_limits (repository, pr_number, user_login, window_started_at, request_count) VALUES (?, ?, ?, ?, 1) ON CONFLICT(repository, pr_number, user_login) DO UPDATE SET window_started_at = CASE WHEN github_mention_limits.window_started_at <= ? THEN ? ELSE github_mention_limits.window_started_at END, request_count = CASE WHEN github_mention_limits.window_started_at <= ? THEN 1 ELSE github_mention_limits.request_count + 1 END WHERE github_mention_limits.window_started_at <= ? OR github_mention_limits.request_count < ?",
            "INSERT INTO github_mention_limits (repository, pr_number, user_login, window_started_at, request_count) VALUES ($1, $2, $3, $4, 1) ON CONFLICT(repository, pr_number, user_login) DO UPDATE SET window_started_at = CASE WHEN github_mention_limits.window_started_at <= $5 THEN $6 ELSE github_mention_limits.window_started_at END, request_count = CASE WHEN github_mention_limits.window_started_at <= $7 THEN 1 ELSE github_mention_limits.request_count + 1 END WHERE github_mention_limits.window_started_at <= $8 OR github_mention_limits.request_count < $9",
        );
        let result = sqlx::query(query)
            .bind(repository)
            .bind(i64::try_from(pr_number).unwrap_or(i64::MAX))
            .bind(user_login.to_ascii_lowercase())
            .bind(now)
            .bind(cutoff)
            .bind(now)
            .bind(cutoff)
            .bind(cutoff)
            .bind(max_requests)
            .execute(&self.pool)
            .await
            .context("failed to apply GitHub mention rate limit")?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn backup_sqlite(&self, destination: &str) -> anyhow::Result<()> {
        if !self.sqlite {
            bail!("online SQLite backups are only available when CURURU_DATABASE_URL uses sqlite:");
        }
        if std::path::Path::new(destination).exists() {
            bail!("backup destination already exists: {destination}");
        }
        sqlx::query("VACUUM INTO ?")
            .bind(destination)
            .execute(&self.pool)
            .await
            .context("SQLite online backup failed")?;
        Ok(())
    }

    const fn sql(&self, sqlite: &'static str, postgres: &'static str) -> &'static str {
        if self.sqlite { sqlite } else { postgres }
    }
}

/// Create the candidate table. A publication interrupted by a restart has an
/// unknown outcome, so it becomes `uncertain` and is never resumed.
async fn initialize_candidates(pool: &AnyPool, sqlite: bool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS hosted_candidates (
                repository TEXT NOT NULL,
                pr_number BIGINT NOT NULL,
                digest TEXT NOT NULL,
                head_sha TEXT NOT NULL,
                check_run_id BIGINT,
                candidate TEXT NOT NULL,
                state TEXT NOT NULL,
                approved_by TEXT,
                created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL,
                PRIMARY KEY (repository, digest)
            )",
    )
    .execute(pool)
    .await
    .context("failed to initialize hosted publication candidates")?;

    let now = epoch_seconds();
    sqlx::query(if sqlite {
            "UPDATE hosted_candidates SET state = 'uncertain', updated_at = ? WHERE state = 'publishing'"
        } else {
            "UPDATE hosted_candidates SET state = 'uncertain', updated_at = $1 WHERE state = 'publishing'"
        })
        .bind(now)
        .execute(pool)
        .await
        .context("failed to recover interrupted publications")?;
    Ok(())
}

fn stored_candidate(row: &sqlx::any::AnyRow) -> anyhow::Result<StoredCandidate> {
    let pr: i64 = sqlx::Row::try_get(row, "pr_number")?;
    let check: Option<i64> = sqlx::Row::try_get(row, "check_run_id")?;
    Ok(StoredCandidate {
        pr_number: u64::try_from(pr).context("invalid stored PR number")?,
        digest: sqlx::Row::try_get(row, "digest")?,
        state: sqlx::Row::try_get(row, "state")?,
        check_run_id: check.and_then(|id| u64::try_from(id).ok()),
        candidate: sqlx::Row::try_get(row, "candidate")?,
    })
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn sqlite_queue_deduplicates_and_claims_once() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("cururu.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = DeliveryStore::connect(&url).await.unwrap();
        assert!(
            store
                .enqueue("delivery-1", "pull_request", "{}")
                .await
                .unwrap()
        );
        assert!(
            !store
                .enqueue("delivery-1", "pull_request", "{}")
                .await
                .unwrap()
        );

        let first = store.claim_next().await.unwrap().unwrap();
        assert_eq!(first.id, "delivery-1");
        assert_eq!(first.attempts, 1);
        assert!(store.claim_next().await.unwrap().is_none());
        store.finish(&first.id).await.unwrap();
        assert!(store.claim_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn sqlite_candidates_supersede_claim_once_expire_and_recover_as_uncertain() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("cururu.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = DeliveryStore::connect(&url).await.unwrap();
        assert!(
            store
                .store_candidate("o/r", 1, "d1", "h1", "{}")
                .await
                .unwrap()
        );
        store
            .set_candidate_check_run("o/r", "d1", 77)
            .await
            .unwrap();
        assert_eq!(
            store
                .find_candidate_by_check_run("o/r", 77)
                .await
                .unwrap()
                .unwrap()
                .digest,
            "d1"
        );
        assert!(
            store
                .has_pending_candidate_for_head("o/r", 1, "h1")
                .await
                .unwrap()
        );
        assert!(
            store
                .store_candidate("o/r", 1, "d2", "h2", "{}")
                .await
                .unwrap()
        );
        assert_eq!(
            store
                .find_candidate("o/r", "d1")
                .await
                .unwrap()
                .unwrap()
                .state,
            "superseded"
        );
        assert!(!store.claim_candidate("o/r", "d1", "alice").await.unwrap());
        assert!(store.claim_candidate("o/r", "d2", "alice").await.unwrap());
        assert!(!store.claim_candidate("o/r", "d2", "bob").await.unwrap());
        // A claimed candidate is not reset by a duplicate preparation.
        assert!(
            !store
                .store_candidate("o/r", 1, "d2", "h2", "{}")
                .await
                .unwrap()
        );
        store
            .finish_candidate("o/r", "d2", "published")
            .await
            .unwrap();
        assert!(!store.claim_candidate("o/r", "d2", "alice").await.unwrap());

        assert!(
            store
                .store_candidate("o/r", 1, "d3", "h3", "{}")
                .await
                .unwrap()
        );
        let late = epoch_seconds() + CANDIDATE_APPROVAL_TTL_SECS + 1;
        assert!(
            !store
                .claim_candidate_at("o/r", "d3", "alice", late)
                .await
                .unwrap()
        );

        assert!(store.claim_candidate("o/r", "d3", "alice").await.unwrap());
        drop(store);
        let store = DeliveryStore::connect(&url).await.unwrap();
        assert_eq!(
            store
                .find_candidate("o/r", "d3")
                .await
                .unwrap()
                .unwrap()
                .state,
            "uncertain"
        );
        assert!(!store.claim_candidate("o/r", "d3", "alice").await.unwrap());
    }

    #[tokio::test]
    async fn sqlite_mention_limit_is_scoped_to_user_and_pull_request() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("cururu.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = DeliveryStore::connect(&url).await.unwrap();

        for _ in 0..5 {
            assert!(
                store
                    .allow_mention_at("owner/repo", 7, "alice", 1_000, 3_600, 5)
                    .await
                    .unwrap()
            );
        }
        assert!(
            !store
                .allow_mention_at("owner/repo", 7, "alice", 1_001, 3_600, 5)
                .await
                .unwrap()
        );
        assert!(
            store
                .allow_mention_at("owner/repo", 8, "alice", 1_001, 3_600, 5)
                .await
                .unwrap()
        );
        assert!(
            store
                .allow_mention_at("owner/repo", 7, "bob", 1_001, 3_600, 5)
                .await
                .unwrap()
        );
        assert!(
            store
                .allow_mention_at("owner/repo", 7, "alice", 4_600, 3_600, 5)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn sqlite_online_backup_creates_a_consistent_database_copy() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("cururu.db");
        let backup_path = dir.path().join("backup.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = DeliveryStore::connect(&url).await.unwrap();
        store
            .enqueue("delivery-2", "issue_comment", "{}")
            .await
            .unwrap();
        store
            .backup_sqlite(backup_path.to_str().unwrap())
            .await
            .unwrap();

        let backup_url = format!("sqlite://{}?mode=ro", backup_path.display());
        sqlx::any::install_default_drivers();
        let backup = AnyPoolOptions::new()
            .max_connections(1)
            .connect(&backup_url)
            .await
            .unwrap();
        let row = sqlx::query(
            "SELECT delivery_id FROM github_deliveries WHERE delivery_id = 'delivery-2'",
        )
        .fetch_optional(&backup)
        .await
        .unwrap();
        assert!(row.is_some());
    }

    #[tokio::test]
    async fn postgres_delivery_queue_round_trips_when_test_database_is_configured() {
        let Ok(database_url) = std::env::var("CURURU_TEST_POSTGRES_URL") else {
            return;
        };
        let store = DeliveryStore::connect(&database_url).await.unwrap();
        let id = format!(
            "integration-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        assert!(store.enqueue(&id, "pull_request", "{}").await.unwrap());
        assert!(!store.enqueue(&id, "pull_request", "{}").await.unwrap());
        let delivery = store.claim_next().await.unwrap().unwrap();
        assert_eq!(delivery.id, id);
        store.finish(&delivery.id).await.unwrap();
        sqlx::query(store.sql(
            "DELETE FROM github_deliveries WHERE delivery_id = ?",
            "DELETE FROM github_deliveries WHERE delivery_id = $1",
        ))
        .bind(&delivery.id)
        .execute(&store.pool)
        .await
        .unwrap();
        let repository = format!(
            "mention-limit-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        for _ in 0..5 {
            assert!(
                store
                    .allow_mention_at(&repository, 42, "alice", 1_000, 3_600, 5)
                    .await
                    .unwrap()
            );
        }
        assert!(
            !store
                .allow_mention_at(&repository, 42, "alice", 1_001, 3_600, 5)
                .await
                .unwrap()
        );
        assert!(
            store
                .allow_mention_at(&repository, 42, "alice", 4_600, 3_600, 5)
                .await
                .unwrap()
        );

        sqlx::query(store.sql(
            "DELETE FROM github_mention_limits WHERE repository = ?",
            "DELETE FROM github_mention_limits WHERE repository = $1",
        ))
        .bind(repository)
        .execute(&store.pool)
        .await
        .unwrap();
    }
}
