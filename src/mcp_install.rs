//! Create project-local MCP configuration only when no file already exists.
use std::io::{Read, Write};
use std::path::Path;

use anyhow::Context;
use clap::ValueEnum;
use serde_json::json;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Client {
    Opencode,
    Codex,
    Cursor,
    Claude,
}

/// Prepared JSONC replacement held under an advisory lock until confirmation.
#[cfg(unix)]
pub struct ActiveJsoncUpdate {
    path: std::path::PathBuf,
    file: std::fs::File,
    original: String,
    merged: String,
    metadata: std::fs::Metadata,
}

#[cfg(unix)]
impl ActiveJsoncUpdate {
    pub fn prepare(client: Client, path: &Path, executable: &str) -> anyhow::Result<Self> {
        use std::os::unix::fs::MetadataExt as _;
        anyhow::ensure!(
            !matches!(client, Client::Codex),
            "Codex uses TOML; this command supports JSONC clients only"
        );
        let metadata = std::fs::symlink_metadata(path).context("cannot inspect configuration")?;
        anyhow::ensure!(
            metadata.file_type().is_file() && metadata.nlink() == 1,
            "configuration must be a regular file without hard links"
        );
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        file.lock().context("cannot lock configuration")?;
        let opened = file.metadata()?;
        anyhow::ensure!(
            opened.is_file()
                && opened.nlink() == 1
                && opened.dev() == metadata.dev()
                && opened.ino() == metadata.ino(),
            "configuration changed during opening"
        );
        let mut original = String::new();
        (&mut file).take(1_048_577).read_to_string(&mut original)?;
        anyhow::ensure!(
            original.len() <= 1_048_576,
            "configuration exceeds size limit"
        );
        let merged = merge_preview(client, &original, executable)?;
        Ok(Self {
            path: path.into(),
            file,
            original,
            merged,
            metadata: opened,
        })
    }

    pub fn apply(mut self, backup: &Path) -> anyhow::Result<()> {
        use std::io::Seek as _;
        use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut staged = tempfile::NamedTempFile::new_in(parent)
            .context("cannot create staged configuration")?;
        staged.write_all(self.merged.as_bytes())?;
        staged
            .as_file()
            .set_permissions(self.metadata.permissions())?;
        staged.as_file().sync_all()?;
        let mut backup_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(backup)
            .context(
                "backup already exists or cannot be created; configuration was not replaced",
            )?;
        backup_file
            .write_all(self.original.as_bytes())
            .context("backup write failed; configuration was not replaced")?;
        backup_file
            .sync_all()
            .context("backup sync failed; configuration was not replaced")?;
        let current = std::fs::symlink_metadata(&self.path)
            .context("configuration disappeared; backup retained")?;
        anyhow::ensure!(
            current.file_type().is_file()
                && current.nlink() == 1
                && current.dev() == self.metadata.dev()
                && current.ino() == self.metadata.ino()
                && current.mode() == self.metadata.mode(),
            "configuration changed; backup retained, nothing replaced"
        );
        self.file.rewind()?;
        let mut bytes = Vec::new();
        (&mut self.file).take(1_048_577).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes == self.original.as_bytes(),
            "configuration changed; backup retained, nothing replaced"
        );
        staged
            .persist(&self.path)
            .map_err(|_| anyhow::anyhow!("replacement failed; backup retained"))?;
        Ok(())
    }
}

/// Verify only the explicit server command, never execute it or print config values.
pub fn check_entry(client: Client, input: &str, executable: &str) -> anyhow::Result<()> {
    if matches!(client, Client::Codex) {
        let value: toml::Value =
            toml::from_str(input).map_err(|_| anyhow::anyhow!("invalid TOML configuration"))?;
        let entry = value
            .get("mcp_servers")
            .and_then(|v| v.get("cururu"))
            .and_then(toml::Value::as_table)
            .context("cururu server not found or not a table")?;
        anyhow::ensure!(
            entry.get("command").and_then(toml::Value::as_str) == Some(executable),
            "cururu command differs from expected executable"
        );
        anyhow::ensure!(
            entry
                .get("args")
                .and_then(toml::Value::as_array)
                .is_some_and(|args| args.len() == 1 && args[0].as_str() == Some("mcp")),
            "cururu arguments differ from expected"
        );
        return Ok(());
    }
    let doc = jsonc_parser::cst::CstRootNode::parse(input, &jsonc_parser::ParseOptions::default())
        .map_err(|_| anyhow::anyhow!("invalid JSONC configuration"))?;
    let root = doc
        .object_value()
        .context("configuration must be an object")?;
    ensure_unique(&root)?;
    let servers = if matches!(client, Client::Opencode) {
        let mcp = root.object_value("mcp").context("mcp object not found")?;
        ensure_unique(&mcp)?;
        mcp.object_value("servers")
            .context("mcp servers object not found")?
    } else {
        root.object_value("mcpServers")
            .context("mcpServers object not found")?
    };
    ensure_unique(&servers)?;
    let entry = servers
        .object_value("cururu")
        .context("cururu server not found or not an object")?;
    ensure_unique(&entry)?;
    let read = |key: &str| -> anyhow::Result<serde_json::Value> {
        let value = entry
            .get(key)
            .and_then(|v| v.value())
            .context("required server field missing")?;
        serde_json::from_str(&value.to_string()).context("unsupported value formatting")
    };
    if matches!(client, Client::Opencode) {
        anyhow::ensure!(
            read("type")? == json!("local") && read("command")? == json!([executable, "mcp"]),
            "cururu server does not match expected local command"
        );
    } else {
        anyhow::ensure!(
            read("type")? == json!("stdio")
                && read("command")? == json!(executable)
                && read("args")? == json!(["mcp"]),
            "cururu server does not match expected stdio command"
        );
    }
    Ok(())
}

/// Write a proposed merge to a new file, never replacing either input or output.
pub fn write_merge_proposal(
    client: Client,
    source: &Path,
    destination: &Path,
    executable: &str,
) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(source).context("failed to inspect configuration")?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "source must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= 1_048_576,
        "configuration exceeds size limit"
    );
    let read_bounded = || -> anyhow::Result<String> {
        let mut text = String::new();
        std::fs::File::open(source)?
            .take(1_048_577)
            .read_to_string(&mut text)?;
        anyhow::ensure!(text.len() <= 1_048_576, "configuration exceeds size limit");
        Ok(text)
    };
    let original = read_bounded().context("failed to read configuration")?;
    anyhow::ensure!(
        original.len() <= 1_048_576,
        "configuration exceeds size limit"
    );
    let merged = merge_preview(client, &original, executable)?;
    let current = read_bounded().context("failed to recheck configuration")?;
    anyhow::ensure!(
        current == original,
        "configuration changed during preparation; no proposal was written"
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(destination)
        .context("proposal already exists or cannot be created; no file was replaced")?;
    file.write_all(merged.as_bytes())
        .context("failed to write proposal; inspect the new file before retrying")?;
    file.flush()
        .context("failed to flush proposal; inspect the new file before retrying")?;
    Ok(())
}

/// Append a validated Codex entry without replacing or reformatting the file.
pub fn update_codex(path: &Path, executable: &str) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path).context("failed to inspect configuration")?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "configuration must be a regular file, not a symlink"
    );
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
        .context("failed to open configuration")?;
    file.lock().context("failed to lock configuration")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "configuration must be a regular file"
    );
    let mut original = String::new();
    (&mut file)
        .take(1_048_577)
        .read_to_string(&mut original)
        .context("failed to read UTF-8 configuration")?;
    anyhow::ensure!(
        original.len() <= 1_048_576,
        "configuration exceeds size limit"
    );
    let merged = merge_preview(Client::Codex, &original, executable)?;
    let suffix = merged
        .strip_prefix(&original)
        .context("merge did not preserve original content")?;
    file.write_all(suffix.as_bytes())
        .context("failed to append configuration; inspect the file before retrying")?;
    file.flush()
        .context("failed to flush configuration; inspect the file before retrying")?;
    Ok(())
}

/// Prepare a merge for inspection; never writes an existing configuration.
pub fn merge_preview(client: Client, original: &str, executable: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        !executable.is_empty() && !executable.chars().any(char::is_control),
        "invalid executable path"
    );
    if matches!(client, Client::Codex) {
        let mut value: toml::Value =
            toml::from_str(original).context("invalid TOML configuration")?;
        let root = value
            .as_table_mut()
            .context("configuration must be a table")?;
        let servers = root
            .entry("mcp_servers")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::default()));
        let servers = servers
            .as_table_mut()
            .context("mcp_servers must be a table")?;
        anyhow::ensure!(
            !servers.contains_key("cururu"),
            "cururu server already exists; refusing conflict"
        );
        let mut entry = toml::map::Map::new();
        entry.insert("command".into(), toml::Value::String(executable.into()));
        entry.insert(
            "args".into(),
            toml::Value::Array(vec![toml::Value::String("mcp".into())]),
        );
        let snippet = toml::to_string(&entry)?;
        let merged = format!("{original}\n[mcp_servers.cururu]\n{snippet}");
        let parsed: toml::Value = toml::from_str(&merged)
            .context("cannot safely append MCP server to this TOML layout")?;
        let mut expected = value;
        expected["mcp_servers"]
            .as_table_mut()
            .context("mcp_servers must be a table")?
            .insert("cururu".into(), toml::Value::Table(entry));
        anyhow::ensure!(
            parsed == expected,
            "TOML merge changed existing configuration"
        );
        return Ok(merged);
    }
    let document =
        jsonc_parser::cst::CstRootNode::parse(original, &jsonc_parser::ParseOptions::default())
            .map_err(|error| anyhow::anyhow!("invalid JSONC configuration: {error}"))?;
    let root = document
        .object_value()
        .context("configuration must be an object")?;
    ensure_unique(&root)?;
    let servers = if matches!(client, Client::Opencode) {
        let mcp = root
            .object_value_or_create("mcp")
            .context("mcp must be an object")?;
        ensure_unique(&mcp)?;
        mcp.object_value_or_create("servers")
            .context("servers must be an object")?
    } else {
        root.object_value_or_create("mcpServers")
            .context("mcpServers must be an object")?
    };
    ensure_unique(&servers)?;
    anyhow::ensure!(
        servers.get("cururu").is_none(),
        "cururu server already exists; refusing conflict"
    );
    let entry = servers
        .object_value_or_create("cururu")
        .context("failed to create server entry")?;
    if matches!(client, Client::Opencode) {
        entry.append("type", "local".into());
        let command = entry
            .array_value_or_create("command")
            .context("failed to create command")?;
        command.append(executable.into());
        command.append("mcp".into());
    } else {
        entry.append("type", "stdio".into());
        entry.append("command", executable.into());
        entry
            .array_value_or_create("args")
            .context("failed to create args")?
            .append("mcp".into());
    }
    let result = document.to_string();
    jsonc_parser::cst::CstRootNode::parse(&result, &jsonc_parser::ParseOptions::default())
        .map_err(|error| anyhow::anyhow!("generated JSONC is invalid: {error}"))?;
    Ok(result)
}

fn ensure_unique(object: &jsonc_parser::cst::CstObject) -> anyhow::Result<()> {
    let mut names = std::collections::HashSet::new();
    for property in object.properties() {
        let name = property.decoded_name().context("invalid property name")?;
        anyhow::ensure!(
            names.insert(name),
            "duplicate configuration key; refusing ambiguous merge"
        );
    }
    Ok(())
}

pub fn install(
    root: &Path,
    client: Client,
    executable: &str,
) -> anyhow::Result<std::path::PathBuf> {
    anyhow::ensure!(
        !executable.is_empty() && !executable.chars().any(char::is_control),
        "invalid executable path"
    );
    if matches!(client, Client::Opencode) {
        anyhow::ensure!(
            !root.join("opencode.json").exists(),
            "opencode.json already exists; merge the MCP entry manually"
        );
    }
    let (path, content) = match client {
        Client::Opencode => (
            "opencode.jsonc",
            serde_json::to_string_pretty(
                &json!({"mcp":{"servers":{"cururu":{"type":"local","command":[executable,"mcp"]}}}}),
            )?,
        ),
        Client::Cursor => (
            ".cursor/mcp.json",
            serde_json::to_string_pretty(
                &json!({"mcpServers":{"cururu":{"type":"stdio","command":executable,"args":["mcp"]}}}),
            )?,
        ),
        Client::Claude => (
            ".mcp.json",
            serde_json::to_string_pretty(
                &json!({"mcpServers":{"cururu":{"type":"stdio","command":executable,"args":["mcp"]}}}),
            )?,
        ),
        Client::Codex => {
            let mut entry = toml::map::Map::new();
            entry.insert("command".into(), toml::Value::String(executable.into()));
            entry.insert(
                "args".into(),
                toml::Value::Array(vec![toml::Value::String("mcp".into())]),
            );
            let mut servers = toml::map::Map::new();
            servers.insert("cururu".into(), toml::Value::Table(entry));
            let mut config = toml::map::Map::new();
            config.insert("mcp_servers".into(), toml::Value::Table(servers));
            (".codex/config.toml", toml::to_string(&config)?)
        }
    };
    let destination = root.join(path);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .context("failed to create client configuration directory")?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .context("configuration already exists or cannot be created; it was not overwritten")?;
    file.write_all(content.as_bytes())
        .context("failed to write MCP configuration")?;
    file.write_all(b"\n")
        .context("failed to finish MCP configuration")?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn active_jsonc_update_preserves_backup_comments_and_rejects_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("mcp.json");
        let backup = dir.path().join("backup.json");
        let original = "{\n // preserve\n \"mcpServers\": {\"other\": {}},\n}";
        std::fs::write(&config, original).unwrap();
        let update =
            ActiveJsoncUpdate::prepare(Client::Claude, &config, "/fixture/cururu").unwrap();
        update.apply(&backup).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        let updated = std::fs::read_to_string(&config).unwrap();
        assert!(updated.contains("// preserve"));
        assert!(updated.contains("\"other\""));
        assert!(check_entry(Client::Claude, &updated, "/fixture/cururu").is_ok());
        assert!(ActiveJsoncUpdate::prepare(Client::Claude, &config, "cururu").is_err());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), updated);
    }

    #[cfg(unix)]
    #[test]
    fn active_jsonc_update_detects_changes_and_never_overwrites_backup() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("mcp.json");
        let backup = dir.path().join("backup.json");
        std::fs::write(&config, "{}").unwrap();
        let update = ActiveJsoncUpdate::prepare(Client::Cursor, &config, "cururu").unwrap();
        std::fs::write(&config, "{\"changed\":true}").unwrap();
        assert!(update.apply(&backup).is_err());
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "{\"changed\":true}"
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{}");
        let update = ActiveJsoncUpdate::prepare(Client::Cursor, &config, "cururu").unwrap();
        assert!(update.apply(&backup).is_err());
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "{\"changed\":true}"
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn active_jsonc_update_refuses_symlinks_and_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.json");
        let symbolic = dir.path().join("symbolic.json");
        let hard = dir.path().join("hard.json");
        std::fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, &symbolic).unwrap();
        assert!(ActiveJsoncUpdate::prepare(Client::Claude, &symbolic, "cururu").is_err());
        std::fs::hard_link(&target, &hard).unwrap();
        assert!(ActiveJsoncUpdate::prepare(Client::Claude, &target, "cururu").is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "{}");
    }

    #[test]
    fn check_entry_rejects_missing_incompatible_and_duplicate_without_echoing_secrets() {
        for client in [
            Client::Opencode,
            Client::Codex,
            Client::Cursor,
            Client::Claude,
        ] {
            let input = if matches!(client, Client::Codex) {
                "secret = 'private-value'"
            } else {
                r#"{"secret":"private-value"}"#
            };
            let error = check_entry(client, input, "cururu")
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private-value"));
        }
        let duplicate = r#"{"mcpServers":{"cururu":{"type":"stdio","command":"cururu","command":"other","args":["mcp"]}}}"#;
        assert!(check_entry(Client::Cursor, duplicate, "cururu").is_err());
        let wrong = r#"{"mcpServers":{"cururu":{"type":"stdio","command":"cururu","args":["mcp","extra"]}}}"#;
        assert!(check_entry(Client::Claude, wrong, "cururu").is_err());
        assert!(
            !check_entry(Client::Codex, "secret = 'private-value", "cururu")
                .unwrap_err()
                .to_string()
                .contains("private-value")
        );
    }

    #[test]
    fn proposal_preserves_source_and_refuses_existing_destinations_and_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("mcp.json");
        let destination = dir.path().join("proposal.json");
        let original = "{\n // keep\n \"mcpServers\": {\"other\": {}},\n}";
        std::fs::write(&source, original).unwrap();
        write_merge_proposal(Client::Cursor, &source, &destination, "cururu").unwrap();
        let proposal = std::fs::read_to_string(&destination).unwrap();
        assert!(proposal.contains("// keep"));
        assert!(proposal.contains("\"other\""));
        assert!(proposal.contains("\"cururu\""));
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        assert!(write_merge_proposal(Client::Cursor, &source, &destination, "other").is_err());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), proposal);
        assert!(
            write_merge_proposal(
                Client::Cursor,
                &destination,
                &dir.path().join("conflict.json"),
                "cururu"
            )
            .is_err()
        );
        assert!(!dir.path().join("conflict.json").exists());
        assert!(write_merge_proposal(Client::Cursor, &source, &source, "cururu").is_err());
        assert_eq!(std::fs::read_to_string(source).unwrap(), original);
    }

    #[test]
    fn jsonc_preview_preserves_comments_and_rejects_conflicts_for_each_client() {
        for client in [Client::Opencode, Client::Cursor, Client::Claude] {
            let input = if matches!(client, Client::Opencode) {
                "{\n // user note\n \"mcp\": {\"servers\": {\"other\": {\"command\": [\"x\"]},}},\n \"theme\": \"dark\",\n}"
            } else {
                "{\n // user note\n \"mcpServers\": {\"other\": {\"command\": \"x\"},},\n \"theme\": \"dark\",\n}"
            };
            let merged = merge_preview(client, input, "/path with spaces/cururu").unwrap();
            assert!(merged.contains("// user note"));
            assert!(merged.contains("\"other\""));
            assert!(merged.contains("\"theme\": \"dark\""));
            assert!(merged.contains("/path with spaces/cururu"));
            assert!(merge_preview(client, &merged, "another").is_err());
            assert!(merge_preview(client, "{\"x\": 1, \"x\": 2}", "cururu").is_err());
        }
    }

    #[test]
    fn codex_update_preserves_comments_and_refuses_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "# preserve me\n[mcp_servers.other]\ncommand = 'other'\n";
        std::fs::write(&path, original).unwrap();
        update_codex(&path, "/opt/cururu").unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.starts_with(original));
        let parsed: toml::Value = toml::from_str(&updated).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["other"]["command"].as_str(),
            Some("other")
        );
        assert_eq!(
            parsed["mcp_servers"]["cururu"]["command"].as_str(),
            Some("/opt/cururu")
        );
        assert!(update_codex(&path, "different").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), updated);
        std::fs::write(&path, "mcp_servers = { other = { command = 'other' } }").unwrap();
        let incompatible = std::fs::read_to_string(&path).unwrap();
        assert!(update_codex(&path, "cururu").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), incompatible);
    }

    #[test]
    fn codex_update_rejects_malformed_and_oversized_files_without_modification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for original in ["invalid = [".to_owned(), "#".repeat(1_048_577)] {
            std::fs::write(&path, &original).unwrap();
            assert!(update_codex(&path, "cururu").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[cfg(unix)]
    #[test]
    fn codex_update_rejects_symlink_without_modifying_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.toml");
        let link = dir.path().join("config.toml");
        let original = "# keep\n";
        std::fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(update_codex(&link, "cururu").is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), original);
    }

    #[test]
    fn merge_preview_preserves_values_and_rejects_conflicts() {
        for client in [
            Client::Opencode,
            Client::Codex,
            Client::Cursor,
            Client::Claude,
        ] {
            let original = match client {
                Client::Codex => "# comment\n[mcp_servers.other]\ncommand = 'other'\n",
                Client::Opencode => {
                    r#"{"mcp":{"servers":{"other":{"type":"local","command":["other"]}}},"theme":"dark"}"#
                }
                _ => r#"{"mcpServers":{"other":{"command":"other"}},"setting":true}"#,
            };
            let merged = merge_preview(client, original, "cururu").unwrap();
            assert!(merged.contains("other"));
            assert!(merged.contains("cururu"));
            if matches!(client, Client::Codex) {
                assert!(merged.starts_with(original));
                assert!(merged.contains("# comment"));
            }
            assert!(merge_preview(client, &merged, "different").is_err());
        }
        let commented = "{ // comment\n \"mcp\": { \"servers\": { \"other\": {}, }, },\n}";
        let merged = merge_preview(Client::Opencode, commented, "cururu").unwrap();
        assert!(merged.contains("// comment"));
        assert!(merged.contains("\"other\""));
        assert!(merge_preview(Client::Opencode, "{\"mcp\":{},\"mcp\":{}}", "cururu").is_err());
        assert!(
            merge_preview(
                Client::Codex,
                "mcp_servers = { other = { command = 'other' } }",
                "cururu"
            )
            .is_err()
        );
    }

    #[test]
    fn creates_valid_client_configs_and_never_overwrites() {
        for client in [
            Client::Opencode,
            Client::Codex,
            Client::Cursor,
            Client::Claude,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = install(dir.path(), client, "/path with spaces/cururu").unwrap();
            let original = std::fs::read_to_string(&path).unwrap();
            assert!(check_entry(client, &original, "/path with spaces/cururu").is_ok());
            assert!(check_entry(client, &original, "other").is_err());
            if matches!(client, Client::Codex) {
                let parsed: toml::Value = toml::from_str(&original).unwrap();
                assert_eq!(
                    parsed["mcp_servers"]["cururu"]["command"].as_str(),
                    Some("/path with spaces/cururu")
                );
            } else {
                let parsed: serde_json::Value = serde_json::from_str(&original).unwrap();
                let entry = if matches!(client, Client::Opencode) {
                    &parsed["mcp"]["servers"]["cururu"]["command"][0]
                } else {
                    &parsed["mcpServers"]["cururu"]["command"]
                };
                assert_eq!(entry, "/path with spaces/cururu");
            }
            assert!(install(dir.path(), client, "other").is_err());
            assert_eq!(std::fs::read_to_string(path).unwrap(), original);
        }
    }
}
