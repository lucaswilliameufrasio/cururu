# Agent rules

## Public repository: no private identifiers

This repository, its PRs, issues, commits and releases are public. Never write
the name, owner, URL or PR/issue number of a private or organization
repository where Cururu runs, nor customer, organization or people's names
taken from those runs, in code, comments, tests, fixtures, docs, changelog
entries, commit messages or PR text.

Describe the case generically instead, for example "a PR with 18 answered and
resolved threads (~15 KB) failed with ..." rather than `owner/repo#123`.

Before committing, check the staged diff and the commit message for such
identifiers (`git diff --cached`); when an incident came from a private
repository, search for its name explicitly.
