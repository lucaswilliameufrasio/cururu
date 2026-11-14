# ReviewBench investigation

ReviewBench provides a 25-PR test set and a 219-PR public full set. An offline
run requires an adapter producing the benchmark's findings contract, a reviewer
container, a model for the review and a separately configured LLM judge. The
official leaderboard uses three rounds of the full set and an additional
submission and approval process. Offline scores are not leaderboard scores.

Sources consulted: [ReviewBench repository](https://github.com/review-bench/ReviewBench),
[offline evaluation](https://github.com/review-bench/ReviewBench/blob/main/docs/OFFLINE_EVAL.md),
and [methodology](https://review-bench.ai/methodology/).

## Adapter

`bench/reviewbench/` packages the released Cururu CLI as an image that follows
the agent contract. It stages the PR as changes against the merge base, so
`cururu api-review` sees the three-dot diff with head line numbers, and
converts its receipt to the findings contract. A failed review exits non-zero
instead of reporting an empty one.

```sh
docker build --platform linux/amd64 -t cururu-reviewbench:dev bench/reviewbench
# from a run directory, with OPENROUTER_API_KEY in the environment:
TRY_AGENT_WORK=<work dir> <ReviewBench>/scripts/try-agent.sh cururu-reviewbench:dev --pr <index> -e OPENROUTER_API_KEY
```

## Run 2026-11-14 (offline, test set, not a leaderboard score)

- Benchmark `d1f751e`, Cururu 5.1.0, review model `openai/gpt-6-luna`
  through OpenRouter, defaults except `max_diff_bytes = 2000000`.
- Coverage: 19 of 25 PRs reviewed. Failures: two chunks with inconsistent
  file coverage discarded the whole review (emdash#2, agents-config#1), two
  single files larger than `review.chunk_bytes` (translator-ingests#336,
  PROTRIDER#10), two binary files in the diff (wirechat#183, fastimage#165).
- Review cost: about USD 0.13 for the 25 PRs (provider-reported).
- Official judge (`claude-sonnet-5` through OpenRouter) on 4 PRs cost about
  USD 3.6 before the key's USD 4 limit stopped it: its classifier explores the
  repository with tools, so it is far costlier than the review. Only two PRs
  completed with it.
- Matching was then done by Claude Code (Claude Sonnet 5.5) with the
  official matcher prompt (`build_judge_inputs.py`, `score_matches.py`).
  Unmatched candidate findings were not verified, so there is no precision
  over all findings, only over matched ones.

| Metric | Value |
| --- | --- |
| Candidate findings | 47 (2.5 per reviewed PR) |
| Golden true positives found (recall, 19 PRs) | 29 of 257 (11.3%) |
| Matched findings that are golden true positives | 21 of 26 (80.8%) |
| Unmatched findings (not verified) | 21 |

Main improvement points:

1. **Robustness on large or mixed PRs.** One inconsistent chunk, one oversized
   file or one binary file makes Cururu refuse the whole PR. Skip and disclose
   unreviewable files, retry or isolate a failing chunk, and split oversized
   files instead of aborting.
2. **Recall.** Cururu reports about 2.5 findings per PR against about 13.5
   real issues per PR in the golden set. Experiments to run with this harness:
   review profile, `policy.minimum_confidence`, `policy.max_findings`, prompt
   guidance, and model choice.
3. **Speculative security findings.** Two findings matched issues the experts
   labelled false positives (a theoretical ReDoS and a stat-then-read race). A valid run
must record the benchmark revision, Cururu revision, models and configuration,
coverage, failures, cost, judge and prompt hashes. A fresh output directory is
needed for every run. Missing cases must not silently disappear from reported
coverage. The public answers and upstream discussions create answer-leakage
risk; follow the benchmark's isolation and network restrictions and disable
hosted search/connectors. The local script alone does not enforce these limits.

Model access, expenditure, container execution and any official submission need
appropriate authorization. Benchmark results alone do not authorize enabling
evaluator filtering or establish representative production performance.
