# ReviewBench investigation

ReviewBench provides a 25-PR test set and a 219-PR public full set. An offline
run requires an adapter producing the benchmark's findings contract, a reviewer
container, a model for the review and a separately configured LLM judge. The
official leaderboard uses three rounds of the full set and an additional
submission and approval process. Offline scores are not leaderboard scores.

Sources consulted: [ReviewBench repository](https://github.com/review-bench/ReviewBench),
[offline evaluation](https://github.com/review-bench/ReviewBench/blob/main/docs/OFFLINE_EVAL.md),
and [methodology](https://review-bench.ai/methodology/).

No Cururu benchmark was run and no score is available. The current hosted-PR
review command is not yet a benchmark adapter: the harness supplies a local
checkout and diff, and expects a separate findings JSON contract. A valid run
must record the benchmark revision, Cururu revision, models and configuration,
coverage, failures, cost, judge and prompt hashes. A fresh output directory is
needed for every run. Missing cases must not silently disappear from reported
coverage. The public answers and upstream discussions create answer-leakage
risk; follow the benchmark's isolation and network restrictions and disable
hosted search/connectors. The local script alone does not enforce these limits.

Model access, expenditure, container execution and any official submission need
appropriate authorization. Benchmark results alone do not authorize enabling
evaluator filtering or establish representative production performance.
