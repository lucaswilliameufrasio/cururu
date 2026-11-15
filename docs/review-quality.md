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

## Two kinds of evidence

| Evidence | What it establishes | What it does not |
| --- | --- | --- |
| `canonical_review_prompt_carries_the_comment_quality_rules` | The prompt carries each rule. | That a model follows it. |
| `cost_is_rendered_even_when_token_usage_is_disabled` (its partial-coverage asserts) | A partial review is not rendered as a clean one. | Anything about the model's own summary. |
| `tests/review_quality/run.py --check` (run by `review_quality_scenarios_reach_the_model_intact`) | Each synthetic scenario builds, and its diff, context and prior feedback reach the model; skipped and truncated files are reported. A loopback fake answers as the model. | Review quality. |
| `tests/review_quality/run.py --send OUT` | Real answers from the configured model for each scenario, written with the scenario's expectations for a human or model judge. | A general quality score: six small synthetic cases, one model, one run. |

Deterministic tests are never evidence of review quality. Only a real run,
judged against the expectations, says anything about it, and only for the
model, configuration and scenarios used.

## Scenarios

`tests/review_quality/scenarios.json` holds six synthetic changes:

| Scenario | Expectation in short |
| --- | --- |
| `concrete-finding-with-alternative` | Path traversal explained with the route that reaches it; an alternative with its rationale, offered as one option; a test. |
| `insufficient-evidence` | A field that might be missing from an unseen module is not asserted as a defect. |
| `pre-existing-versus-regression` | String-built SQL that predates the change is labelled as pre-existing. |
| `deploy-partial-failure` | Old workers read a renamed column; no assumed rollback or guaranteed reordering. |
| `resolved-prior-comment` | A timeout already fixed after earlier feedback is not reported again. |
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
