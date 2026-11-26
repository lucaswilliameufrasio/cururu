//! Read-only access to a local Git checkout, so local reviews can load the
//! same repository context as hosted reviews. Context comes from committed
//! revisions (`HEAD`), never from uncommitted working-tree files.

use crate::scm::ScmProvider;
use anyhow::Context;
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Largest repository file read as context.
const MAX_CONTEXT_FILE_BYTES: usize = 1024 * 1024;
/// Most tree entries accepted, matching the GitHub tree API's limit; a larger
/// tree fails instead of producing an incomplete context selection.
const MAX_TREE_ENTRIES: usize = 100_000;
/// Bound on the raw `ls-tree` output for [`MAX_TREE_ENTRIES`] entries.
const MAX_TREE_BYTES: usize = 32 * 1024 * 1024;
/// Bound on small metadata answers such as a SHA or an object size.
const MAX_METADATA_BYTES: usize = 128;
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LocalGit {
    root: PathBuf,
    /// The trusted base revision for context, when it is not `HEAD`: the
    /// merge base of a reviewed commit range.
    base: Option<String>,
}

impl LocalGit {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            base: None,
        }
    }

    /// Read context at `base` (a full commit SHA) instead of `HEAD`.
    pub fn with_base(root: impl Into<PathBuf>, base: String) -> Self {
        Self {
            root: root.into(),
            base: Some(base),
        }
    }

    /// Run git without blocking the async runtime, reading at most
    /// `max_bytes` of output within [`GIT_TIMEOUT`].
    async fn git(&self, args: &[&str], max_bytes: usize) -> anyhow::Result<Vec<u8>> {
        let command = args.first().copied().unwrap_or_default();
        let mut child = Command::new("git")
            .current_dir(&self.root)
            .args([
                "-c",
                "core.quotePath=false",
                "-c",
                "core.fsmonitor=false",
                "--no-pager",
            ])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to run git")?;
        let mut stdout = child.stdout.take().context("git output is unavailable")?;
        let mut output = Vec::new();
        let limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let finished = tokio::time::timeout(GIT_TIMEOUT, async {
            (&mut stdout).take(limit).read_to_end(&mut output).await?;
            if output.len() > max_bytes {
                return Ok(None);
            }
            child.wait().await.map(Some)
        })
        .await;
        // Dropping `child` on an early return kills the process.
        let status = match finished {
            Err(_) => anyhow::bail!("git {command} timed out"),
            Ok(Err(error)) => return Err(error).context("failed to read git output"),
            Ok(Ok(None)) => anyhow::bail!("git {command} output exceeds {max_bytes} bytes"),
            Ok(Ok(Some(status))) => status,
        };
        anyhow::ensure!(status.success(), "git {command} failed");
        Ok(output)
    }

    /// A repository-relative path: no empty, `.` or `..` components, so it
    /// cannot leave the tree or be resolved against the working directory.
    fn valid_path(path: &str) -> bool {
        path.split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
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
    async fn fetch_config_toml(&self, revision: &str) -> anyhow::Result<Option<String>> {
        Self::valid_revision(revision)?;
        let path = ".cururu.toml";
        let entry = self.git(&["ls-tree", revision, "--", path], 256).await?;
        if entry.is_empty() {
            return Ok(None);
        }
        self.fetch_file_at_ref(path, revision).await.map(Some)
    }

    async fn fetch_head_sha(&self) -> anyhow::Result<String> {
        Ok(String::from_utf8(
            self.git(&["rev-parse", "--verify", "HEAD"], MAX_METADATA_BYTES)
                .await?,
        )?
        .trim()
        .to_string())
    }

    async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
        anyhow::bail!("local context access does not provide change-request diffs")
    }

    /// Local changes are reviewed against `HEAD`, which is therefore the
    /// base; a commit range is reviewed against its merge base.
    async fn fetch_base_sha(&self) -> anyhow::Result<String> {
        if let Some(base) = &self.base {
            return Ok(base.clone());
        }
        self.fetch_head_sha().await
    }

    async fn list_repository_paths_at_revision(
        &self,
        revision: &str,
    ) -> anyhow::Result<Vec<String>> {
        Self::valid_revision(revision)?;
        let raw = self
            .git(
                &["ls-tree", "-r", "-z", "--full-tree", revision],
                MAX_TREE_BYTES,
            )
            .await?;
        // Entries are `<mode> <type> <object>\t<path>`; only files are kept,
        // as with the GitHub tree API (submodules are commits, not blobs).
        let entries: Vec<&[u8]> = raw
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .collect();
        anyhow::ensure!(
            entries.len() <= MAX_TREE_ENTRIES,
            "local repository tree has more than {MAX_TREE_ENTRIES} entries; context matching cannot be complete"
        );
        Ok(entries
            .into_iter()
            .filter_map(|entry| {
                let tab = entry.iter().position(|byte| *byte == b'\t')?;
                let (meta, path) = (&entry[..tab], &entry[tab + 1..]);
                let is_blob = meta.split(|byte| *byte == b' ').nth(1) == Some(b"blob".as_slice());
                is_blob.then(|| String::from_utf8(path.to_vec()).ok())?
            })
            .collect())
    }

    async fn fetch_file_at_ref(&self, path: &str, revision: &str) -> anyhow::Result<String> {
        Self::valid_revision(revision)?;
        anyhow::ensure!(Self::valid_path(path), "invalid repository path");
        let object = format!("{revision}:{path}");
        let size: usize = String::from_utf8(
            self.git(&["cat-file", "-s", &object], MAX_METADATA_BYTES)
                .await?,
        )?
        .trim()
        .parse()
        .context("invalid git object size")?;
        if size > MAX_CONTEXT_FILE_BYTES {
            return Err(UnreadableFile::TooLarge.into());
        }
        String::from_utf8(
            self.git(&["cat-file", "blob", &object], MAX_CONTEXT_FILE_BYTES)
                .await?,
        )
        .map_err(|_| UnreadableFile::NotText.into())
    }
}

/// A committed file that exists but cannot be read as review text.
#[derive(Debug, thiserror::Error)]
pub enum UnreadableFile {
    #[error("repository file is too large for context")]
    TooLarge,
    #[error("repository file is not UTF-8 text")]
    NotText,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
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
        // A submodule is a commit entry in the tree, not a file.
        let commit = String::from_utf8(
            std::process::Command::new("git")
                .current_dir(dir.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let gitlink = format!("160000,{},vendor/sub", commit.trim());
        git(
            dir.path(),
            &["update-index", "--add", "--cacheinfo", &gitlink],
        );
        git(dir.path(), &["commit", "-q", "-m", "submodule"]);
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
        for invalid in [
            "../etc/passwd",
            "src/../lib.rs",
            "./src/lib.rs",
            "/src/lib.rs",
            "src//lib.rs",
            "",
        ] {
            assert!(!LocalGit::valid_path(invalid), "{invalid}");
        }
        assert!(LocalGit::valid_path("docs/guia..md"));
        let object = format!("{head}:src/lib.rs");
        assert!(
            local
                .git(&["cat-file", "blob", &object], 4)
                .await
                .unwrap_err()
                .to_string()
                .contains("exceeds 4 bytes")
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
