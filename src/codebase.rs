//! Whole-codebase review input: the committed files at one revision, shown to
//! the review pipeline as a diff from the empty tree so chunking, anchors,
//! validation and context are reused unchanged.

use crate::scm::ScmProvider;
use std::fmt::Write as _;

/// Default bound on the file content collected for one codebase review.
pub const DEFAULT_MAX_BYTES: usize = 1_000_000;
/// Most files read for one codebase review; empty and binary files do not
/// count toward the byte bound but still cost a read each.
pub const MAX_FILES: usize = 5_000;

/// The selection is larger than `codebase.max_bytes`.
#[derive(Debug, thiserror::Error)]
#[error(
    "the selected files exceed codebase.max_bytes ({limit} bytes); select fewer paths (for example --path 'src/**') or raise the limit"
)]
pub struct CodebaseTooLarge {
    pub limit: usize,
}

/// One file of the selection.
#[derive(Debug)]
pub enum CodebaseFile {
    Text {
        path: String,
        content: String,
    },
    /// Binary content, shown as a binary change so the pipeline reports it as
    /// not reviewed.
    Binary {
        path: String,
    },
    /// A path that cannot be written into a diff header safely (a control
    /// character such as a newline, or ` b/`); left out and reported.
    Skipped {
        path: String,
    },
}

/// Whether `path` can be written into a `diff --git` header unambiguously.
fn header_safe(path: &str) -> bool {
    !path.chars().any(char::is_control) && !path.contains(" b/")
}

/// Diff that adds every file to an empty tree. Empty files are left out:
/// they have nothing to review.
#[must_use]
pub fn diff_from_empty_tree(files: &[CodebaseFile]) -> String {
    let mut diff = String::new();
    for file in files {
        match file {
            CodebaseFile::Skipped { .. } => {}
            CodebaseFile::Binary { path } => {
                let _ = write!(
                    diff,
                    "diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n"
                );
            }
            CodebaseFile::Text { path, content } => {
                if content.is_empty() {
                    continue;
                }
                let lines: Vec<&str> = content.split_inclusive('\n').collect();
                let _ = write!(
                    diff,
                    "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
                    lines.len()
                );
                for line in &lines {
                    diff.push('+');
                    diff.push_str(line);
                }
                if !content.ends_with('\n') {
                    diff.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }
    diff
}

/// The files of a codebase review and the paths `review.ignore` left out.
#[derive(Debug)]
pub struct Selection {
    pub files: Vec<CodebaseFile>,
    pub ignored: Vec<String>,
}

/// Collect the selected committed files at `revision` through any SCM
/// adapter (a local checkout or the hosting API). Fails when the content
/// exceeds `max_bytes`, so a large repository is never sent by surprise.
pub async fn collect(
    scm: &dyn ScmProvider,
    revision: &str,
    patterns: &[String],
    ignore: &globset::GlobSet,
    max_bytes: usize,
) -> anyhow::Result<Selection> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern).map_err(|error| {
            anyhow::anyhow!("invalid codebase path pattern {pattern}: {error}")
        })?);
    }
    let selection = builder.build()?;
    let (mut paths, mut ignored): (Vec<String>, Vec<String>) = scm
        .list_repository_paths_at_revision(revision)
        .await?
        .into_iter()
        .filter(|path| patterns.is_empty() || selection.is_match(path))
        .partition(|path| !ignore.is_match(path));
    paths.sort();
    ignored.sort();
    anyhow::ensure!(
        !paths.is_empty(),
        "no committed file matches the codebase selection"
    );
    anyhow::ensure!(
        paths.len() <= MAX_FILES,
        "the selection has {} files, more than {MAX_FILES}; select fewer paths (for example --path 'src/**')",
        paths.len()
    );
    let mut total = 0usize;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        if !header_safe(&path) {
            files.push(CodebaseFile::Skipped { path });
            continue;
        }
        let content = scm.fetch_file_at_ref(&path, revision).await;
        let file = match content {
            // The hosting API decodes content lossily, so U+FFFD marks bytes
            // that were not UTF-8; such a file is reported as not reviewed.
            Ok(content) if content.contains('\0') || content.contains('\u{FFFD}') => {
                CodebaseFile::Binary { path }
            }
            Ok(content) => {
                total = total.saturating_add(content.len());
                if total > max_bytes {
                    return Err(CodebaseTooLarge { limit: max_bytes }.into());
                }
                CodebaseFile::Text { path, content }
            }
            // A local file that is not UTF-8 or is too large to read is
            // reported as not reviewed instead of failing the whole review.
            Err(error) if error.is::<crate::local_git::UnreadableFile>() => {
                CodebaseFile::Binary { path }
            }
            Err(error) => return Err(error),
        };
        files.push(file);
    }
    Ok(Selection { files, ignored })
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
    async fn collects_selected_committed_files_and_enforces_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@example.invalid"]);
        git(dir.path(), &["config", "user.name", "t"]);
        for (path, content) in [
            ("src/lib.rs", b"pub fn a() {}\n".as_slice()),
            ("src/gen/out.rs", b"generated\n"),
            ("docs/guide.md", b"# Guide\n"),
            ("assets/logo.png", b"\x89PNG\x00\x01"),
            ("odd\nname.txt", b"text\n"),
        ] {
            let file = dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, content).unwrap();
        }
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        // Uncommitted content is never part of the review.
        std::fs::write(dir.path().join("src/lib.rs"), "uncommitted\n").unwrap();
        let local = crate::local_git::LocalGit::new(dir.path());
        let head = crate::scm::ScmProvider::fetch_head_sha(&local)
            .await
            .unwrap();
        let ignore = {
            let mut builder = globset::GlobSetBuilder::new();
            builder.add(globset::Glob::new("**/gen/**").unwrap());
            builder.build().unwrap()
        };
        let names = |files: &[CodebaseFile]| -> Vec<String> {
            files
                .iter()
                .map(|file| match file {
                    CodebaseFile::Text { path, .. } => path.clone(),
                    CodebaseFile::Binary { path } => format!("{path} (binary)"),
                    CodebaseFile::Skipped { path } => format!("{path} (skipped)"),
                })
                .collect()
        };

        let selection = collect(&local, &head, &[], &ignore, 1000).await.unwrap();
        assert_eq!(selection.ignored, ["src/gen/out.rs"]);
        let all = selection.files;
        assert_eq!(
            names(&all),
            [
                "assets/logo.png (binary)",
                "docs/guide.md",
                "odd\nname.txt (skipped)",
                "src/lib.rs"
            ]
        );
        assert!(
            matches!(&all[3], CodebaseFile::Text { content, .. } if content == "pub fn a() {}\n")
        );

        let src = collect(&local, &head, &["src/**".into()], &ignore, 1000)
            .await
            .unwrap()
            .files;
        assert_eq!(names(&src), ["src/lib.rs"]);

        let error = collect(&local, &head, &[], &ignore, 10).await.unwrap_err();
        assert!(
            error.downcast_ref::<CodebaseTooLarge>().is_some(),
            "{error:#}"
        );
        assert!(
            collect(&local, &head, &["nothing/**".into()], &ignore, 1000)
                .await
                .is_err()
        );
    }

    #[test]
    fn files_become_a_diff_from_the_empty_tree_with_every_line_anchorable() {
        let files = [
            CodebaseFile::Text {
                path: "src/a.rs".into(),
                content: "fn a() {}\nfn b() {}\n".into(),
            },
            CodebaseFile::Text {
                path: "src/empty.rs".into(),
                content: String::new(),
            },
            CodebaseFile::Text {
                path: "README".into(),
                content: "no newline".into(),
            },
            CodebaseFile::Binary {
                path: "logo.png".into(),
            },
            CodebaseFile::Skipped {
                path: "evil\ndiff --git a/x b/x".into(),
            },
        ];
        let diff = diff_from_empty_tree(&files);
        let parsed = crate::diff::parse_unified_diff(&diff);
        let lines: Vec<(&str, Vec<u32>)> = parsed
            .iter()
            .map(|file| (file.path.as_str(), file.right_lines.clone()))
            .collect();
        assert_eq!(
            lines,
            [
                ("src/a.rs", vec![1, 2]),
                ("README", vec![1]),
                ("logo.png", vec![])
            ]
        );
        assert!(!diff.contains("src/empty.rs"));
        assert!(diff.contains("+no newline\n\\ No newline at end of file\n"));
        assert!(diff.contains("Binary files /dev/null and b/logo.png differ"));
        assert!(
            !diff.contains("evil"),
            "an unsafe path never reaches a diff header"
        );
        assert!(header_safe("src/a b.rs") && !header_safe("a b/c") && !header_safe("x\ny"));
    }
}
