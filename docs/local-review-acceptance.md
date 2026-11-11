# Local review journey acceptance map

This map covers the agreed Cururu principal review journeys, not desktop or
live-model acceptance. “Local evidence” means synthetic inputs, local Git/SQLite
and loopback service fakes. A test can establish the exercised case, not model
quality, complete coverage by an agent, or compatibility with a real provider.
The checkout containing this document is uncommitted; rerun the checks after
changes rather than treating this as evidence for another revision.

| Requirement and observable boundary | Implementation | Local evidence and scope |
| --- | --- | --- |
| CLI local and PR preparation identifies the scope and snapshot without calling a model or publishing. Local scope excludes untracked files. | `src/main.rs`: `PrepareReview`, `prepare_review_input`; `src/review_input.rs` | `local_preparation_reads_tracked_changes_not_untracked_files`; `prepare_review_previews_explicit_sarif_without_invoking_an_agent`; `tests/api_pr_smoke.py`. |
| CLI inference uses the user's configured provider and key, and refuses without `--confirm-send`. An expected snapshot, when supplied, must match before sending. | `src/main.rs`: `ApiReview`; `src/config/provider.rs`; `src/anthropic.rs`; `crates/cururu-engine/src/openai.rs` | `api_review_requires_consent_and_uses_configured_api_for_local_diff`; `api_review_anthropic_uses_native_messages_only_after_consent`; `configured_compatible_providers_send_requested_model_to_selected_endpoint`; `provider_from_name`. Real provider accounts are not tested. |
| CLI rejects stale input or evidence after inference and never publishes as part of `api-review`. | `src/main.rs`: `prepare_api_review`, `ApiReview`; `src/review_input.rs`: `assemble_review` | `api_review_pr_confirms_and_revalidates_without_publication` / `tests/api_pr_smoke.py`; `api_review_sarif_requires_consent_and_revalidates_both_files`; `api_review_multiple_chunks_fails_closed_for_late_failure_and_evidence_change`. |
| MCP host owns its authentication and disclosure; Cururu's stdio tools prepare, validate and assemble, and expose no model invocation or publication tool. | `src/main.rs`: `Mcp`; `src/mcp.rs`: `serve`, `handle` | `initialization_tool_schema_and_invalid_calls_follow_json_rpc`; `stdio_server_prepares_local_fixture_without_non_protocol_stdout`; `stdio_multichunk_requires_all_results_in_order`. The host's actual consent UX and agent review are outside Cururu's control and not established by these tests. |
| CLI and MCP reject incomplete chunks, stale snapshots, invalid anchors, empty/binary/submodule or oversized review input; structural validation is not an approval or proof of review. | `src/review_input.rs`: `ensure_reviewable`, `validate_chunk_review`, `assemble_review`; `src/mcp.rs`: `validate`, `assemble` | `chunk_review_rejects_inconsistent_reported_coverage`; `validation_and_assembly_reject_unreviewable_diffs`; `stdio_multichunk_requires_all_results_in_order`; `tests/api_pr_smoke.py`. |
| Collected analyzer findings are promoted subject to policy; model/agent-claimed analyzer provenance is stripped. Suggestions are retained only when enabled. | `src/review_input.rs`: `assemble_review`; `src/agent.rs`: candidate and policy handling; `src/review.rs` | `assembly_removes_model_claimed_provenance_and_executable_suggestion`; `stdio_server_prepares_local_fixture_without_non_protocol_stdout`; `hosted_review_does_not_trust_model_analyzer_provenance`; `hosted_suggestions_follow_configured_policy`; `tests/publication_pty.py`. Explicit SARIF selection is supported by CLI, not as an MCP tool argument. |
| Selected SARIF and manifest are bounded, bound to the reviewed HEAD and re-read. A matching HEAD does not prove analysis of local uncommitted changes. | `src/review_input.rs`: `attach_local_analysis`; `src/analysis.rs` | `explicit_sarif_requires_matching_manifest_and_binds_input`; `api_review_sarif_requires_consent_and_revalidates_both_files`; `local_evidence_is_required_and_rechecked_before_posting`. |
| Publishing a CLI or MCP PR receipt is separate, requires an interactive confirmation, previews the exact advisory comment, and revalidates target, configuration, evidence and snapshot. | `src/main.rs`: `PublishAgentReview`; `src/agent_publication.rs`: `preview`, `publish` | `publication_with_sarif_requires_interactive_confirmation` / `tests/publication_pty.py`; `publish_with_local_evidence_refuses_piped_confirmation`; `preview_and_publication_reject_stale_or_unrelated_receipts`; `preview_obeys_suggestion_policy_and_escapes_replacements`. The GitHub POST cannot be made atomic with the preceding snapshot check. |
| Hosted reviews reject incomplete output, stale diff/head, altered context, feedback, analysis, or trusted repository/shared configuration. | `src/review.rs`: `run_review`; `src/main.rs`: `Review`; `src/app.rs`: `run_pull_request_review` | `hosted_review_does_not_publish_when_feedback_or_evidence_changes` / `tests/hosted_review_smoke.py`; `hosted_multichunk_review_rejects_second_chunk_failure`; `hosted_review_rejects_head_changed_after_model_response`; `signed_webhook_revalidates_config_after_http_evaluator`. The last test uses a test-build-only loopback evaluator endpoint. |
| Evaluator filtering remains disabled; `observe` is restricted to explicitly allowlisted repositories and cannot publish a stale result after evaluation. | `src/review.rs`: `ensure_evaluator_mode_allowed`, `run_review`; `src/app.rs`: `github_context`; `src/config/mod.rs`: `authorize_evaluator_for_repository` | `filtering_is_rejected_before_scm_or_provider_access`; `signed_webhook_filter_stops_before_external_side_effects`; `signed_webhook_observe_completes_and_rejects_changed_config`; `signed_webhook_revalidates_config_after_http_evaluator`. No thresholds, filtering rollout or real evaluator call is authorized by these tests. |
| MCP client configuration changes do not silently overwrite existing settings; active JSONC update needs interactive approval and backup. | `src/main.rs`: MCP install/update commands; `src/mcp_install.rs` | `client_mcp_install_and_check_cli_are_read_only_and_fail_on_mismatch`; `codex_mcp_update_cli_preserves_existing_configuration_and_refuses_conflicts`; `jsonc_update_requires_interactive_approval_and_makes_private_backup`; `jsonc_update_refuses_noninteractive_confirmation`. Cooperative locking does not stop non-cooperating writers; metadata preservation is limited as documented. |
| Retired direct OpenCode `agent-review` sends nothing. | `src/main.rs`: `AgentReview` | `retired_opencode_review_never_contacts_server`. |

## Deliberate differences and limits

* `api-review` requires per-invocation `--confirm-send`; MCP relies on the
  invoking host for disclosure consent. These are not equivalent consent UIs.
* The existing GitHub Action/App automation publishes as configured; it does
  **not** require an interactive confirmation for each run. The separate
  `PUBLISH` confirmation is the CLI/MCP receipt path, not the hosted path.
  Repository owners must explicitly configure and authorize hosted automation.
* Hosted review combines collected analyzer evidence after model inference; `api-review`
  includes selected evidence in its model input; MCP exposes collected evidence
  to the host. API and MCP do not invoke the optional evaluator. These are
  documented behavior differences, not evidence of feature parity.
* All receipts and file counts remain editable or self-reported; none proves
  that a model or agent actually read every file. Hosted publication also has a
  non-atomic check-to-POST interval.
* Real model quality, real-provider compatibility, actual agent-host disclosure
  UX and user acceptance remain unverified and outside the authorized local run.

## Reproduction

Run `cargo fmt --check`, `cargo test --workspace --offline --quiet --
--test-threads=1`, and `cargo clippy --workspace --all-targets --offline -- -D
warnings` in an isolated environment with the cached Rust dependencies, Python,
Git and OpenSSL. The HTTP tests use loopback fakes and generated fixture keys;
they do not require actual credentials. The optional real-client MCP connection
smokes and their limitations are documented in `agent-adapter-boundaries.md`.
