# Self-hosted GitHub App

Cururu's GitHub App server is operated by the repository owner. It receives
GitHub webhooks, verifies their signatures, stores deliveries in a durable queue,
and processes reviews asynchronously. The Action remains available for users who
do not need a continuously running service.

Use either the Action or the GitHub App for a given repository, not both at the
same time. Each mode has a distinct GitHub identity and owns its own review
comments. When switching an open PR from Action to App, a collaborator can run
`/cururu review` once to publish the App's formal review for the current head.

## Create and install the App

Create a GitHub App in the account/organization that owns the installation:

- **Webhook URL:** `https://cururu.example.com/api/webhooks/github`
- **Webhook secret:** generate a unique random value and set it as
  `GITHUB_WEBHOOK_SECRET` in the deployment.
- **Repository permissions:** Metadata (read, automatic), Contents (read), Pull
  requests (write), Issues (write). Cururu reads repository config/context and
  diffs, writes reviews/comments, and answers comments/mentions.
- Add Checks (read) only when enabling analyzer evidence from GitHub Check Runs.
- **Subscribe to events:** Pull request (`opened`, `synchronize`, `reopened`,
  `ready_for_review`), Issue comment (`created`), Pull request review comment
  (`created`).
- Install the App on the repositories where Cururu should review code.

Download the App's private key and store it as a file available to the service;
do not commit it. The App's installation token is generated at runtime and is
never written to the config or database.

For a private shared configuration, install the App on both the consumer and
base-config repositories. With all-repository access in an organization,
changing to another base repository in that installation needs no App change.
Selective installations require an administrator to add each base repository.
For a base in a different organization/account, install the App there too; Cururu
obtains that installation's read token for the pinned config fetch.

## Run with PostgreSQL

```sh
cp .env.example .env
# Set the App ID/slug, private-key file, webhook secret, LLM key, and a strong
# CURURU_DB_PASSWORD in .env. Keep .env and the key out of source control.
docker compose up -d --build
```

The included `compose.yaml` runs Cururu and PostgreSQL. Expose port 8080 behind
the included Nginx reverse proxy and register the public
`/api/webhooks/github` URL in the App settings. Nginx listens only on
`127.0.0.1:18082`, forwards to Cururu's loopback-only port 8080, limits request
bodies to 1 MB, and keeps access logs on stdout.

### Expose with Tailscale Funnel

On a host joined to a tailnet with Funnel enabled by its admin, publish Nginx's
local port:

```sh
sudo tailscale funnel --bg 18082
tailscale funnel status
```

Tailscale terminates public HTTPS; the webhook URL is
`https://<machine>.<tailnet>.ts.net/api/webhooks/github`. Replace the placeholders
with the DNS name shown by `tailscale status`. If you use a regular public domain
instead, point DNS and ports 80/443 at the host and configure Nginx with a
trusted TLS certificate.

`GET /health` is a liveness endpoint. Configure the App's webhook URL only after
the HTTPS endpoint is reachable from GitHub.

## Run with SQLite

Use SQLite for a single Cururu instance when a separate database service is not
wanted:

```sh
docker compose --env-file .env -f compose.sqlite.yaml up -d --build
```

`compose.sqlite.yaml` mounts a persistent volume at `/data`; the database uses
WAL mode and full synchronous durability. While Cururu is running, create a
consistent online backup with:

```sh
docker compose --env-file .env -f compose.sqlite.yaml \
  run --rm cururu backup-sqlite /data/cururu-backup.db
```

The backup destination must not already exist. For continuous off-host
replication, Litestream can run as a sidecar against the same `/data/cururu.db`
volume; configure its replica and credentials separately, and restore the file
before starting Cururu. Do not run the SQLite deployment with multiple Cururu
replicas.

Run one Cururu worker process per App deployment for now; queue delivery IDs are
deduplicated across restarts, but PR-level parallel review locking is not yet a
horizontal-scaling feature. PostgreSQL can still be a managed/separate database
for a single Cururu worker.

## Environment

| Variable | Required | Purpose |
|---|---:|---|
| `GITHUB_APP_ID` | yes | GitHub App identifier used to sign short-lived JWTs |
| `GITHUB_APP_SLUG` | yes | App URL slug used for `@cururu` mention matching |
| `GITHUB_APP_PRIVATE_KEY_PATH` or `GITHUB_APP_PRIVATE_KEY` | yes | App RSA PEM key file or PEM contents |
| `GITHUB_WEBHOOK_SECRET` | yes | HMAC-SHA256 webhook signature verification |
| `LLM_API_KEY` | yes | Provider credential, supplied only as a deployment secret |
| `CURURU_DATABASE_URL` | yes | `postgres://...` / `postgresql://...` or `sqlite:///...` |
| `GITHUB_API_URL` | no | GitHub Enterprise API base URL; defaults to `https://api.github.com` |
| `GITHUB_SERVER_URL` | no | GitHub Enterprise web URL; defaults to `https://github.com` |
| `CURURU_HOST`, `PORT` | no | Bind host and HTTP port; defaults `0.0.0.0:8080` |

The App requests the bot account as a reviewer when a PR is opened. GitHub's
review-request endpoint is documented for user and team accounts; if GitHub
rejects the App bot, Cururu logs that outcome and still submits a formal
`COMMENT` review under its App identity. Comments and review events remain
advisory and do not approve, block, or merge the PR.

If `CURURU_DB_PASSWORD` contains URL-reserved characters, percent-encode them
when constructing `CURURU_DATABASE_URL`.

## Delivery processing

Webhook signatures are checked before JSON parsing or queueing. `X-GitHub-Delivery`
is the queue idempotency key. Workers process one delivery at a time per Cururu
instance, retry failures with bounded backoff, recover interrupted jobs, and
prune completed payloads after seven days. PR-head code is never checked out or
executed. Comment-triggered LLM responses are limited to write-authorized
collaborators, and comment/diff text remains untrusted prompt data.

The same worker reviews PR open/update events, accepts exact `/cururu review`
and `/cururu review --full` commands, answers `@cururu` / `@cururu[bot]` mentions,
and replies in a review-comment thread when a collaborator responds to a Cururu
finding.
