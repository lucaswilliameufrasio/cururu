//! Read-only access to a local Git checkout, so local reviews can load the
//! same repository context as hosted reviews. Context comes from committed
//! revisions (`HEAD`), never from uncommitted working-tree files.

use crate::scm::ScmProvider;
use anyhow::Context;
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Command;

/// Largest repository file read as context.
const MAX_CONTEXT_FILE_BYTES: u64 = 1024 * 1024;

pub struct LocalGit {
    root: PathBuf,
}

impl LocalGit {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn git(&self, args: &[&str]) -> anyhow::Result<Vec<u8>> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args([
                "-c",
                "core.quotePath=false",
                "-c",
                "core.fsmonitor=false",
                "--no-pager",
            ])
            .args(args)
            .output()
            .context("failed to run git")?;
        anyhow::ensure!(
            output.status.success(),
            "git {} failed",
            args.first().copied().unwrap_or_default()
        );
        Ok(output.stdout)
    }

    fn valid_revision(revision: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid local revision"
        );
        Ok(())
    }
}

#[async_trait]
impl ScmProvider for LocalGit {
    async fn fetch_head_sha(&self) -> anyhow::Result<String> {
        Ok(
            String::from_utf8(self.git(&["rev-parse", "--verify", "HEAD"])?)?
                .trim()
                .to_string(),
        )
    }

    async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
        anyhow::bail!("local context access does not provide change-request diffs")
    }

    /// Local changes are reviewed against `HEAD`, which is therefore the base.
    async fn fetch_base_sha(&self) -> anyhow::Result<String> {
        self.fetch_head_sha().await
    }

    async fn list_repository_paths_at_revision(
        &self,
        revision: &str,
    ) -> anyhow::Result<Vec<String>> {
        Self::valid_revision(revision)?;
        let raw = self.git(&[
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            "--full-tree",
            revision,
        ])?;
        Ok(raw
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .filter_map(|path| String::from_utf8(path.to_vec()).ok())
            .collect())
    }

    async fn fetch_file_at_ref(&self, path: &str, revision: &str) -> anyhow::Result<String> {
        Self::valid_revision(revision)?;
        anyhow::ensure!(
            !path.is_empty() && !path.starts_with('/') && !path.contains(".."),
            "invalid repository path"
        );
        let object = format!("{revision}:{path}");
        let size: u64 = String::from_utf8(self.git(&["cat-file", "-s", &object])?)?
            .trim()
            .parse()
            .context("invalid git object size")?;
        anyhow::ensure!(
            size <= MAX_CONTEXT_FILE_BYTES,
            "repository file is too large for context"
        );
        String::from_utf8(self.git(&["cat-file", "blob", &object])?)
            .context("repository file is not UTF-8 text")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(dir)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    #[tokio::test]
    async fn reads_committed_files_not_working_tree_changes() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@example.invalid"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "committed\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("src/lib.rs"), "uncommitted\n").unwrap();

        let local = LocalGit::new(dir.path());
        let head = local.fetch_base_sha().await.unwrap();
        assert_eq!(head.len(), 40);
        assert_eq!(
            local
                .list_repository_paths_at_revision(&head)
                .await
                .unwrap(),
            ["src/lib.rs"]
        );
        assert_eq!(
            local.fetch_file_at_ref("src/lib.rs", &head).await.unwrap(),
            "committed\n"
        );
        assert!(
            local
                .fetch_file_at_ref("../etc/passwd", &head)
                .await
                .is_err()
        );
        assert!(
            local
                .fetch_file_at_ref("src/lib.rs", "HEAD;rm")
                .await
                .is_err()
        );
    }
}
