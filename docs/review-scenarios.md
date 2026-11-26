# Review scenarios

`tests/review_scenarios/run.py` runs a real `cururu` binary through scenarios
taken from reported failures, against a loopback GitHub fake. Each scenario
states the expected behavior and fails when Cururu does not show it. Nothing
is published: every GitHub call goes to the fake.

| Scenario | What it checks |
|---|---|
| `resolved-overflow` | 60 answered, resolved threads over 20 files, 8 of them in the diff. Every decision reaches the model, in full or summarized; in-diff threads first; the log reports `resolved_summarized`. With a real model, counts findings that reopen a decision on the same file. |
| `feedback-race` | A thread is resolved during the model call: the review is still published, without the finding on that thread. A push during the call still fails without publishing. |
| `local-shared-base` | `prepare-review --local` with `[config] base`: the base is read with `CURURU_SHARED_CONFIG_TOKEN` and merged under the repository file. |
| `mcp-repository-path` | The MCP server starts outside the repository: `repository_path`, `base`/`head` (a stacked range) and the error for a directory that is not a Git repository. |
| `mcp-assemble-errors` | `assemble_review` refusals name the missing chunk, the invalid field and a stale snapshot. |
| `mcp-agent` | With `--agent`: opencode itself is the MCP client and reviews a stacked range through the tools. |
| `push-cost` | Model input bytes per push with `incremental` off, `true` (both review the whole PR diff on a new push) and `"changed_files"` (later pushes send only the file they added), and that the README documents it. |
| `findings-cap` | The prompt does not make `max_findings` a target, and the cap keeps the most severe and then most confident findings. With a real model, reports the count against the cap. |

## Running

```sh
cargo build --bin cururu
# Deterministic, free: a fake model answers.
python3 tests/review_scenarios/run.py target/debug/cururu
# One scenario, with evidence JSON per scenario.
python3 tests/review_scenarios/run.py target/debug/cururu --only resolved-overflow --out /tmp/scenarios
```

With a real model, each model call is answered by `opencode run` with no
tools, so it uses whatever login opencode has for that provider (an OpenAI
subscription login uses no API credit):

```sh
python3 tests/review_scenarios/run.py target/debug/cururu \
  --model opencode:openai/gpt-6-luna --agent opencode:openai/gpt-6-luna \
  --only resolved-overflow --only findings-cap --only mcp-agent --out /tmp/scenarios
```

opencode runs `--standalone` in a temporary directory with its own
`opencode.json` (an agent with every tool denied; for `mcp-agent`, only the
Cururu MCP tools allowed). It reads its project from `PWD`, so the runner sets
it. Real-model numbers vary between runs; the deterministic run is the
regression gate, and the real-model run is evidence for review quality.

## Results on 2026-11-26 (`openai/gpt-6-luna`)

| Scenario | 6.3.0 behavior | After the fixes |
|---|---|---|
| `resolved-overflow` | 20 of 60 decisions sent, 40 dropped. 8 findings reopened a decision: 6 on the same file (for example "Chamada HTTP sem timeout" on two files whose decision was dropped), 2 on the same topic elsewhere | All 60 decisions sent (full or summarized); 0 findings reopened a decision |
| `findings-cap` | — | 1 finding with `max_findings = 5` (the shell injection); the cap was not filled |
| `mcp-agent` | `prepare_review` refused `repository_path`; no review | opencode prepared the stacked range and `assemble_review` accepted it |

Real-model numbers vary between runs; one run per row.
