---
name: testing-quality-gate
description: >-
  Choose the right unit and integration tests, add regression coverage for bugs,
  and verify a complete change before calling it done. Use when adding or
  reviewing tests, fixing bugs, changing persistence or APIs, or finishing any
  code change. Run locally available infrastructure for real; fake only
  dependencies that cannot run locally.
---

# Testing strategy and quality gate

Tests should demonstrate that the integrated system works, not only that mocks
agree with the implementation. Follow repository commands and architecture;
these are portable defaults for deciding test boundaries and verifying a
change.

## Select test boundaries

### Unit tests

Use unit tests for a component's logic in isolation. Replace dependencies that
cannot reasonably run locally with fakes or mocks. Cover success, validation,
timeouts, retries, malformed/missing external data, client/server failures, and
partial failures where relevant. Inject clocks or other nondeterministic inputs
when they affect behavior.

### Integration tests

Use real local infrastructure when it can run in the development/test
environment: databases, caches, queues, filesystems, and the application's own
services. Exercise real migrations, queries, transactions, serialization, and
concurrency. Do not replace a local database with an in-memory substitute in an
integration test.

For a dependency that cannot run locally (for example, a third-party SaaS),
start a local server that speaks the real protocol and point the production
client at it. Test actual request serialization, headers, authentication,
timeouts, response parsing, and error mapping at that boundary.

Never point tests at production or shared staging systems. Integration tests
must be isolated, repeatable, and safe to run concurrently: create unique data,
clean up, and avoid fixed identities or order-dependent assertions.

## Regression and coverage

- Every bug fix needs a regression test that fails for the reported defect
  before the fix and passes afterward.
- Test behavior, persistence read-back, authorization, transactions/rollback,
  side effects, and concurrent single-winner/claim-once behavior where
  applicable.
- Coverage may omit entrypoints, generated code, and mechanical bootstrap
  wiring when documented. Do not exclude business logic, authorization,
  validation, handlers, repositories, adapters, or error/retry behavior merely
  to raise a percentage.
- A high coverage percentage does not replace testing meaningful error paths.

## Full quality gate

Before declaring a coding task done, run the changed repository's complete,
fresh checks—not just tests for edited files:

1. Build the project using its documented command.
2. Run the formatter/check in check mode over the repository.
3. Run the repository-wide lint target.
4. Run the full unit test suite without relying on cached results.
5. Run the full integration suite against real local dependencies when the
   repository provides one; start those dependencies and migrations as needed.
6. Run documented security/static checks relevant to the project.

Do not invent commands if the repository defines targets. Re-run required gates
after the final code change. If a check fails, distinguish changed-code failures
from pre-existing failures and report exactly what ran; never claim the suite is
green when only a subset passed.

## Framework-specific tests

For frameworks such as SvelteKit, exercise handlers through the real router and
use local containers for dependencies that are available in Docker. Keep unit
tests for pure behavior and use protocol fakes only for external services.
Follow the repository's test directory and framework conventions rather than
imposing a universal folder layout.
