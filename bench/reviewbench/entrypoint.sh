#!/usr/bin/env bash
# ReviewBench agent contract adapter: review the PR mounted at /work/repo with
# `cururu api-review` and write the findings file at $RB_OUT.
#
# Configuration (manifest `configuration` entries arrive as RB_CONFIG_*):
#   RB_CONFIG_MODEL      model id (default openai/gpt-6-luna)
#   RB_CONFIG_PROVIDER   Cururu provider (default openrouter)
# Credentials: OPENROUTER_API_KEY (or LLM_API_KEY).
set -euo pipefail

: "${RB_OUT:?}" "${RB_HEAD:?}" "${RB_BASE:?}" "${RB_PR_NUMBER:?}" "${RB_PR_JSON:?}"
agent="${RB_AGENT:-cururu}"
model="${RB_CONFIG_MODEL:-openai/gpt-6-luna}"
provider="${RB_CONFIG_PROVIDER:-openrouter}"
export LLM_API_KEY="${LLM_API_KEY:-${OPENROUTER_API_KEY:-}}"
: "${LLM_API_KEY:?set OPENROUTER_API_KEY or LLM_API_KEY}"
export CURURU_PROVIDER="$provider" LLM_MODEL="$model"
# Benchmark PRs can be large; review the whole diff in bounded chunks.
export CURURU_MAX_DIFF_BYTES="${RB_CONFIG_MAX_DIFF_BYTES:-2000000}"
export CURURU_LANGUAGE="${RB_CONFIG_LANGUAGE:-en-US}"
echo "cururu=$(cururu --version | awk '{print $2}') provider=$provider model=$model max_diff_bytes=$CURURU_MAX_DIFF_BYTES" >&2

cd /work/repo
git config --global --add safe.directory /work/repo
# Present the PR as staged changes against the merge base: the index and
# working tree hold the head tree, so finding lines refer to the file at head,
# and the staged diff equals the three-dot diff GitHub shows.
merge_base="$(git merge-base "$RB_BASE" "$RB_HEAD")"
git checkout --quiet --detach "$RB_HEAD"
git reset --quiet --soft "$merge_base"

receipt="$(mktemp)"
if ! cururu api-review --confirm-send >"$receipt"; then
    echo "cururu review failed for PR $RB_PR_NUMBER; no findings file written" >&2
    exit 1
fi
# Usage and cost for the run record (provider-reported cost when available).
jq -c '{usage: .usage, findings: (.review.findings | length)}' "$receipt" >&2

jq --slurpfile pr "$RB_PR_JSON" --arg agent "$agent" '{
    pr: ($pr[0] | {repo, pr_number, base, head}),
    agent: $agent,
    findings: [.review.findings[] | {
        file: .path,
        start_line: (.line // 1),
        end_line: (.line // 1),
        message: ((if (.title | length) > 0 then .title + ": " else "" end) + .message),
        producer: $agent
    }]
}' "$receipt" >"$RB_OUT"
