# Review comment quality

Cururu's review comments should help the author fix the problem without
inventing context or imposing a single solution. The rules live in one place,
the canonical prompt `prompts/review.md`, which every mode uses (Action, App,
`api-review` and MCP). For each supported finding it asks for:

1. the concrete scenario in which the problem happens and its consequence;
2. a viable alternative and why it resolves the problem;
3. that alternative offered as one option the author may replace;
4. the test that would verify the fix, when that adds value.

It also asks the model to check callers and contracts before claiming a
scenario is possible, to separate regressions from pre-existing problems, to
keep impact within the evidence, to express doubt through `confidence` instead
of asserting a hypothesis, to consider old and new versions running together
and partial failure in deployments without assuming a rollback, not to repeat
resolved feedback, and not to describe a partial review as complete.

Cururu's own text follows the same limits: the summary does not call findings
high-confidence (the policy's `minimum_confidence`, 0.65 by default, decides
what is kept), and a review that left files out never reads as "No issues
found."; it lists the files that were not fully reviewed.

## Cross-chunk references

A change larger than one chunk is reviewed chunk by chunk, so a model can
claim that something is unused or never called when its callers were sent in
another chunk (PR #55 got such a finding). Each chunk of a multi-chunk change
therefore ends with a list (at most 25 entries, 3 per name) of where the names
it defines appear in the other chunks, as `path:line` and the line. Chunking
keeps room for it (an eighth of `review.chunk_bytes`, at most 4 KB), so an
annotated chunk stays within `chunk_bytes`; when the room is short, every name
gets one mention before any gets a second, and a call is listed before another
definition of the same name.
Definitions are found without a language parser: a line that starts, after
modifiers such as `pub`, `pub(in path)` or `export`, with a keyword such as `fn`, `def`,
`function`, `func`, `class`, `struct`, `enum`, `trait`, `interface` or `type`.
Names shorter than four characters are left out. The prompt asks the model to
check this list before claiming something is unused. It is a hint, not a
call graph: methods defined without a keyword (JavaScript class methods, Go
methods with a receiver) are not listed.

## Two kinds of evidence

| Evidence | What it establishes | What it does not |
| --- | --- | --- |
| `canonical_review_prompt_carries_the_comment_quality_rules` | The prompt carries each rule. | That a model follows it. |
| `cost_is_rendered_even_when_token_usage_is_disabled` (its partial-coverage asserts) | A partial review is not rendered as a clean one. | Anything about the model's own summary. |
| `tests/review_quality/run.py --check` (run by `review_quality_scenarios_reach_the_model_intact`) | Each synthetic scenario builds, and its diff, context and prior feedback reach the model; skipped and truncated files are reported. A loopback fake answers as the model. | Review quality. |
| `tests/review_quality/run.py --send OUT` | Real answers from the configured model for each scenario, written with the scenario's expectations for a human or model judge. | A general quality score: seven small synthetic cases, one model, one run. |

Deterministic tests are never evidence of review quality. Only a real run,
judged against the expectations, says anything about it, and only for the
model, configuration and scenarios used.

## Scenarios

`tests/review_quality/scenarios.json` holds seven synthetic changes:

| Scenario | Expectation in short |
| --- | --- |
| `concrete-finding-with-alternative` | Path traversal explained with the route that reaches it; an alternative with its rationale, offered as one option; a test. |
| `insufficient-evidence` | A field that might be missing from an unseen module is not asserted as a defect. |
| `pre-existing-versus-regression` | String-built SQL that predates the change is labelled as pre-existing. |
| `deploy-partial-failure` | Old workers read a renamed column; no assumed rollback or guaranteed reordering. |
| `resolved-prior-comment` | A timeout already fixed after earlier feedback is not reported again. |
| `resolved-decision-kept` | A broad `except` the author answered as intentional and resolved is not reported again on the same code. In `--check`, the fake model repeats the finding and Cururu must drop it as `suppressed_as_decided`. |
| `incomplete-review` | Truncated and binary files are reported, and nothing claims full coverage. |

## Running a real evaluation

`--send` uses the provider from the usual environment (`CURURU_PROVIDER`,
`LLM_BASE_URL`, `LLM_API_KEY`, `LLM_MODEL`) and costs one review call per
chunk. Each scenario runs through a loopback GitHub fake and `api-review --pr`,
so nothing is published.

```sh
cargo build
python3 tests/review_quality/run.py target/debug/cururu --send /tmp/review-quality
```

Judge each `OUT/<scenario>.json` against its `expectations`, and record the
model, the Cururu revision and the verdicts with the result.

## Recorded runs

2026-11-15, `openai/gpt-6-luna` through OpenRouter, default configuration
(`minimum_confidence` 0.65). The baseline is the v5.1.1 prompt (main at
`55e7f7e`); "new" is this prompt. Counts are runs in which the behaviour was
seen. The first comparison was judged blind by Claude Code with Sonnet 5.5;
the later runs were read against the same expectations. Six small cases and a
handful of runs per version: treat this as a direction, not a score.

| Scenario | v5.1.1 prompt | New prompt (final revision) |
| --- | --- | --- |
| `concrete-finding-with-alternative` | Finds it (1/1); no test, imperative fix. | Finds it (2/2); cites the route, names a test, offers the fix as one option with a second alternative. |
| `insufficient-evidence` | Asserts duplicate receipts on an unseen retry in 2/4 runs. | Same in 1/2 runs (3/5 across all revisions). Not solved. |
| `pre-existing-versus-regression` | No finding (1/1). | No finding (2/2). The case does not discriminate. |
| `deploy-partial-failure` | Finds the old-worker window in 1/4 runs. | 1/2 runs (3/5 across all revisions); offers expand-and-contract as one option, no assumed rollback. |
| `resolved-prior-comment` | Does not repeat it (1/1). | Does not repeat it (2/2). |
| `incomplete-review` | False finding: reads the split removal as an emptied file (1/1). | No finding (2/2) after removals stay with their replacement; the receipt lists the truncated and binary files. |

What the runs changed in Cururu itself: a hunk is no longer split between a
removal and the addition replacing it, since the model reliably read the bare
removal as deleted content even with a part note. What stays open: a model
still asserts hypotheses that depend on unseen callers or retries, which a
prompt alone did not fix.

ReviewBench rerun (same day, same model, `minimum_confidence` 0.5 and
automatic context, as in the earlier E3 experiment) of the four pull requests
that had been cut off with the broadened prompt alone: two completed, and the
matcher (Claude Code, Sonnet 5.5, official matcher prompt) found 6 of their 42
labelled true positives against 5 for E3 on the same two. One still ran out of
output tokens with only 2.5 KB of answer, so the model's reasoning, not the
findings, used the budget; the other failed because the model twice placed a
finding on a line outside the diff, which aborted the review. Both now have
a fallback: a cut-off chunk is requested once more with twice the output
limit, and a finding still outside the diff after a retry is dropped and
reported instead of failing the review.

With those fallbacks (`fa6482e`), the two failed pull requests completed: one
chunk stopped at the limit and its retry with twice the limit finished; the
model placed no finding outside the diff this time, so dropping was not
exercised by a real run. The matcher found 17 of their 52 labelled true
positives (32.7% recall, matched precision 83.3%) against 15 (28.8%, 86.7%)
for E3 on the same two. Across the four pull requests, Cururu now completes
all of them; E3 also completed them, by chance on one of the two failures.
