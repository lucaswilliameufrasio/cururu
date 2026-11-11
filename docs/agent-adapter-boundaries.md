# Agent adapter boundaries

This note records the evidence available while implementing local-agent review;
it does not claim that an adapter is safe or that a client was tested with a
real model. Client behavior can change between versions.

The former experimental `agent-review` OpenCode server transport has been
retired: the command refuses to send, including when confirmation is supplied.
CLI reviews use configured API credentials; agents initiate their own reviews
through MCP using their own authentication and permissions.

## Claude Code (documentation checked 2026-11-08)

The [CLI reference](https://code.claude.com/docs/en/cli-reference.md) says
`--tools ""` disables built-in tools but does not itself disable MCP tools.
`--safe-mode` retains normal authentication and managed policy, but managed
policy-configured hooks may still run. `--restricted` limits tools and loads
managed settings; it is not described as suppressing managed hooks.

The [headless documentation](https://code.claude.com/docs/en/headless.md)
states that non-interactive mode without `--bare` can load project hooks and
MCP servers even before workspace trust. `--bare` skips discovered hooks and
MCP servers, but still exposes built-in tools unless restricted, and does not
use the user's subscription login or read OAuth/keychain credentials.

Consequently none of these flags alone proves the requested combination of
subscription authentication, no automatic command execution, preserved managed
policy and no agent tools. Cururu does not invoke Claude Code directly yet.
Do not work around this by copying credentials, disabling managed policy or
claiming a prompt-only instruction is an isolation boundary. MCP setup is a
separate host-initiated integration described in `agent-mcp-setup.md`.

### Local MCP connection smoke test

On 2026-11-08, Claude Code 2.1.319 was run with a disposable HOME and project
inside a network-isolated Linux namespace. A local-scope stdio server was
registered using `claude mcp add --transport stdio --scope local cururu --
/cururu mcp`; `claude mcp get cururu` reported `Status: ✔ Connected`. Both
executables and the temporary configuration were isolated from the user's home;
no credentials, model request or real repository were used. This validates
registration and connection only, not tool discovery, a review journey,
authentication, costs, or direct CLI orchestration.

### Manual tool-call limitation

In a separate network-isolated environment with an empty temporary HOME,
`claude mcp --help` for version 2.1.319 listed configuration, status, login,
logout, removal and `serve`, but no manual tool-call command. `serve` starts a
Claude Code MCP server; it is not a documented command for invoking a tool on
an arbitrary configured MCP server. The official
[MCP guide](https://code.claude.com/docs/en/mcp) documents tool invocation by
Claude during a session, but this investigation did not identify an official
manual call interface like Codex app-server's. No model turn or tool invocation
was attempted. The previous connection check must not be interpreted as proof
that Claude Code can complete the review journey without a model.

## Codex MCP configuration smoke test

On 2026-11-08, Codex CLI 0.166.0 was run with a disposable `CODEX_HOME` and
project in a network-isolated Linux namespace. `codex mcp add cururu -- /cururu
mcp` succeeded, and `codex mcp get cururu` reported `enabled: true`, `transport:
stdio`, command `/cururu` and argument `mcp`. The global scope in that output
was confined to the disposable `CODEX_HOME`; no user's settings were changed.
This only verifies registration and readback, not a server connection, tool
discovery, model use, authentication or safe direct CLI orchestration.

On 2026-11-08, a separate network-isolated Codex 0.166.0 app-server smoke test
used the disposable MCP configuration and Cururu 4.9.0 binary. After JSONL
`initialize` and `initialized`, `mcpServerStatus/list` returned Cururu's server
identity and all three tools: `prepare_review`, `validate_review`, and
`assemble_review`, with `toolsError: null`. No thread or turn was started and
no credentials or model were used. This verifies connection and discovery in
that environment, not tool invocation, a review journey, or safe subprocess
orchestration with a user's managed settings.

On 2026-11-08, a further isolated test created an ephemeral Codex thread without
starting a model turn, then called `prepare_review` through
`mcpServer/tool/call` against a disposable Git repository. The response had
`isError: false` and contained the expected synthetic `code.txt` diff. The
network namespace, empty disposable HOME and synthetic repository excluded real
credentials and source code. This verifies one manual tool invocation only;
`validate_review` and `assemble_review` were not invoked through Codex, and
the test does not establish a safe direct agent adapter or a model review.

In a subsequent isolated fixture run, the same Codex version invoked
`validate_review` and `assemble_review` through `mcpServer/tool/call` on an
ephemeral thread. Both accepted a synthetic empty review against the prepared
snapshot; after the fixture diff changed, both rejected that old snapshot. No
model turn, credentials, real repository, or external network was used. This
does not validate model behavior or direct subprocess orchestration.

To repeat this smoke test on Linux with Python 3, Git, bubblewrap and both
executables already installed, run:

```sh
python3 tests/codex_mcp_smoke.py --codex /absolute/path/to/codex --cururu /absolute/path/to/cururu
```

It creates a disposable home and Git fixture, runs in a network-isolated
bubblewrap namespace, and does not pass the user's environment or credentials.
It requires a host where unprivileged bubblewrap namespaces are available; it
is an opt-in compatibility smoke test, not a substitute for the workspace test
suite or an authenticated model review.

### App-server initialization smoke test

On 2026-11-08, Codex 0.166.0 `app-server` received the documented JSONL
`initialize` request in a network-isolated namespace with an empty temporary
HOME and `CODEX_HOME`. It returned a successful response for the request with
`platformFamily: unix` and `platformOs: linux`. No thread or turn was started,
no user credentials or repository were available, and no model was called.
This confirms only initialization of that binary and protocol version. The
[app-server documentation](https://developers.openai.com/codex/app-server/)
describes capabilities including tool execution and hooks; this handshake does
not establish a no-tool/no-hook boundary or preservation of managed policy.
Consequently it does not authorize enabling a direct adapter.

## Cursor CLI execution boundary

The [CLI parameters reference](https://cursor.com/docs/cli/reference/parameters)
describes `--print` as having access to all tools, including write and shell.
The [permissions reference](https://cursor.com/docs/cli/reference/permissions)
also says print mode can use write and shell tools. The
[headless guide](https://cursor.com/docs/cli/headless) says `--force` enables
direct file changes, whereas the [usage guide](https://cursor.com/docs/cli/using)
states that non-interactive mode has full write access. These descriptions do
not establish a reliable no-tool boundary for the versions in use.

[Cursor hooks](https://cursor.com/docs/hooks) can execute project, user or
managed scripts during the agent lifecycle. Sandboxing command execution and
asking the model not to use tools are not equivalent to disabling all hooks and
tools while retaining managed policies and the existing authentication. No
direct Cursor subprocess adapter is enabled on this evidence. The host-driven
MCP integration remains separate and subject to the client's own permissions.
