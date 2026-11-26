use anyhow::{Context, ensure};
use clap::ValueEnum;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Target {
    Claude,
    Codex,
    Grok,
    Opencode,
}

pub fn render() -> String {
    let workflow =
        include_str!("../prompts/skill.md").replace("{{version}}", env!("CARGO_PKG_VERSION"));
    format!("{workflow}\n{}", cururu_engine::DEFAULT_REVIEW_PROMPT)
}

pub fn install(target: Target, global: bool) -> anyhow::Result<()> {
    let root = if global {
        std::env::home_dir().context("cannot determine home directory")?
    } else {
        std::env::current_dir()?
    };
    let agent = match (target, global) {
        (Target::Claude, _) => PathBuf::from(".claude"),
        (Target::Codex, _) => PathBuf::from(".agents"),
        (Target::Grok, _) => PathBuf::from(".grok"),
        (Target::Opencode, false) => PathBuf::from(".opencode"),
        (Target::Opencode, true) => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| root.join(".config"))
            .join("opencode"),
    };
    let dir = root.join(agent).join("skills/cururu-review");
    if !global {
        for ancestor in dir.ancestors().take_while(|p| *p != root) {
            ensure!(
                !ancestor.is_symlink(),
                "refusing symlink: {}",
                ancestor.display()
            );
        }
    }
    fs::create_dir_all(&dir)?;
    write_skill(&dir)
}

fn write_skill(dir: &Path) -> anyhow::Result<()> {
    let path = dir.join("SKILL.md");
    let content = render();
    let mut next = tempfile::NamedTempFile::new_in(dir)?;
    next.write_all(content.as_bytes())?;
    match next.persist_noclobber(&path) {
        Ok(_) => println!("Installed {}", path.display()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            let next = error.file;
            ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "expected a regular SKILL.md"
            );
            let file = fs::File::open(&path)?;
            file.lock()?;
            let original = fs::read_to_string(&path)?;
            if original == content {
                println!("Already current: {}", path.display());
                return Ok(());
            }
            let version_line = format!("  version: \"{}\"", env!("CARGO_PKG_VERSION"));
            if !original
                .lines()
                .take_while(|line| !line.starts_with('#'))
                .any(|line| line == version_line)
            {
                eprintln!(
                    "Skill version differs from cururu {}.",
                    env!("CARGO_PKG_VERSION")
                );
            }
            let status = Command::new("git")
                .args([
                    "--no-pager",
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--",
                ])
                .arg(&path)
                .arg(next.path())
                .status()?;
            ensure!(status.code() == Some(1), "cannot preview skill changes");
            eprint!("Type UPDATE to replace this file: ");
            io::stderr().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            ensure!(answer.trim_end() == "UPDATE", "update cancelled");
            ensure!(
                fs::read_to_string(&path)? == original,
                "skill changed since preview"
            );
            next.persist(&path)?;
            println!("Updated {}", path.display());
        }
        Err(error) => return Err(error.error.into()),
    }
    Ok(())
}
