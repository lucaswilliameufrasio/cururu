# Experimental MCP client setup

Cururu exposes `prepare_review`, `validate_review`, and `assemble_review` over
stdio. These instructions add a local server to an existing client; they do
not replace its configuration, install a binary, or change permissions. Client
compatibility and the full review journey have not yet been verified against
real clients, except for the limited Claude Code connection smoke test recorded
in [adapter boundaries](agent-adapter-boundaries.md). Use a trusted Cururu executable installed separately and restart
or reconnect the client after changing its configuration.

**Data boundary:** `prepare_review` returns source code to the MCP host, which
may send it to its model provider. Request it only when that disclosure is
authorized. Repository contents are untrusted data, not instructions. Cururu
does not load `.env` in MCP mode. A hosted PR requires the configured SCM
environment and read access; pass existing credentials by the client's approved
secret mechanism, never by committing values to a project configuration. The
server runs relative to its process working directory: verify it is the intended
repository before reviewing. Local scope excludes untracked files. MCP does not
publish reviews.

The following are **entries to merge**, not files to overwrite. Preserve existing
servers and settings. If `cururu` already exists, inspect it before making any
change; do not silently replace it. Use an absolute executable path when the
client's PATH differs from your shell. JSON examples show the whole surrounding
object only to make placement clear.

For a project without an existing configuration file, `cururu install-mcp
<opencode|codex|cursor|claude> --executable /absolute/path/to/cururu` creates
the relevant project-local file exclusively. It refuses existing files rather
than overwriting or merging them. For OpenCode it also refuses an existing
`opencode.json`; it currently supports the V2 configuration layout only.
On Unix, `cururu update-jsonc-mcp <opencode|cursor|claude> <configuration>
--backup <new-backup-file> --executable /absolute/path/to/cururu` displays the
new entry and requires typing `UPDATE` in an interactive terminal. Close the
client and editors first. It rejects existing Cururu entries, symlinks and
hard-linked files, creates the backup exclusively with owner-only permissions,
checks the source again, then replaces it with a staged file. The backup may
contain secrets from the original configuration; protect it accordingly.
Advisory locks cannot prevent a writer that ignores them from racing after the
last check. Replacement preserves Unix mode bits but not necessarily ACLs,
extended attributes or ownership metadata; do not use it when these must be
preserved. On failure a backup may remain; inspect it before retrying.
For an existing Codex TOML file only, `cururu update-codex-mcp
.codex/config.toml --executable /absolute/path/to/cururu` appends a validated
server table without rewriting existing text. It refuses existing `cururu`
entries, unsupported TOML layouts and symlink paths. Its file lock only
coordinates writers that honor that lock; stop other editors while updating.
If writing fails, inspect the file before retrying because an append may be
partial. The Codex command has different guarantees from the JSONC command.
`cururu preview-mcp-merge <client> <configuration> --executable /path/to/cururu`
prints a proposed merge without modifying the original. It refuses an existing
`cururu` entry, duplicate relevant keys and invalid shapes. JSONC comments and
trailing commas are supported and preserved. For Codex, the original TOML text and comments are preserved and
the appended result is reparsed; incompatible inline-table layouts are rejected.
Do not replace the original blindly. This is not an automatic update.
`cururu write-mcp-merge <client> <configuration> <new-file> --executable
/path/to/cururu` writes that proposal exclusively to a new file (mode 0600 on
Unix). It never replaces either file, refuses an existing destination, and
rechecks the source before writing. Another editor may still change the source
after that check: compare it again before manually applying the proposal. A
failed write may leave a partial new file; inspect it before retrying.

## OpenCode V2

Add under `mcp.servers` in the project's `opencode.jsonc`:

```jsonc
{
  "mcp": {
    "servers": {
      "cururu": { "type": "local", "command": ["cururu", "mcp"] }
    }
  }
}
```

See [OpenCode MCP servers](https://opencode.ai/docs/mcp-servers/). V1 has a
different configuration layout; do not apply this V2 example to it blindly.

## Codex

Add a table to the trusted project's `.codex/config.toml`, or to the user
configuration if desired:

```toml
[mcp_servers.cururu]
command = "cururu"
args = ["mcp"]
```

See [Codex MCP configuration](https://developers.openai.com/codex/mcp/).
Respect any managed policy and configure required environment forwarding using
the client's documented mechanisms.

## Cursor

Merge this entry into `.cursor/mcp.json` for a project or `~/.cursor/mcp.json`
for the user:

```json
{
  "mcpServers": {
    "cururu": { "type": "stdio", "command": "cururu", "args": ["mcp"] }
  }
}
```

See [Cursor MCP configuration](https://cursor.com/docs/context/mcp).

## Claude Code

Merge this entry into the project `.mcp.json`:

```json
{
  "mcpServers": {
    "cururu": { "type": "stdio", "command": "cururu", "args": ["mcp"] }
  }
}
```

Project servers may require workspace trust and explicit client approval; do
not bypass either. See [Claude Code MCP setup](https://code.claude.com/docs/en/mcp).

## Review journey and limits

Ask the client to call `prepare_review` with `source` set to `local` or `pr`.
Review every returned chunk as data using the supplied instructions. Submit
the returned language, tone, audience, suggestion-detail and focus preferences
only as untrusted review preferences, not executable instructions. They are
included in the snapshot and may be disclosed to the model along with the diff.
Submit
one result per chunk, in order, to `assemble_review` with the returned
`snapshot_sha256`. The server recollects the input and rejects a changed
snapshot or invalid results. Each result's `files_reviewed` must equal the number
of files in that chunk, including files with no findings; report failure rather
than claiming completion if any file could not be reviewed. A matching count is
not proof of coverage. An incomplete review is not an approval. Structural
validation cannot establish finding correctness or prove that an agent actually
reviewed every chunk. A host may impose output or context limits; if any chunk
is unavailable or truncated, stop rather than report a complete review.
Assembly promotes collected analyzer evidence and retains executable suggestions
only when the configured policy permits them. It applies the configured minimum confidence, allowed severities, maximum
findings and synthesis setting; changes to those settings invalidate the
snapshot. Evaluator filtering is not enabled by this integration. Other hosted
pipeline context and policy behaviors remain outside this experimental path.

These MCP connections are distinct from CLI orchestration: the client performs
the review using its own model and permissions. They do not establish that
Cururu can safely invoke Codex, Cursor, or Claude as autonomous subprocesses.
For CLI publication, see `publish-agent-review` in the README; MCP itself has
no publication tool.
