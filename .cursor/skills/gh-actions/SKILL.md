---
name: gh-actions
description: GitHub Actions best practices — current action versions, caching, security, and common patterns. Activate when writing or modifying GitHub Actions workflows.
---

# GitHub Actions Best Practices

When writing or modifying GitHub Actions workflows (`.github/workflows/*.yml`), follow these guidelines.

## Action Versions

Never guess action versions. Before writing or updating a workflow, check the latest release for each action you use:

```bash
gh api repos/{owner}/{action}/releases/latest --jq '.tag_name'
```

The `{owner}/{action}` maps directly to the GitHub repo — e.g. `actions/checkout` lives at `github.com/actions/checkout`.

For example:
```bash
gh api repos/actions/checkout/releases/latest --jq '.tag_name'
gh api repos/actions/setup-node/releases/latest --jq '.tag_name'
```

Use the latest major version (e.g. if the latest tag is `v6.3.0`, use `v6`). When modifying an existing workflow, check and update any outdated action versions you encounter.

## Security

- Always set top-level `permissions` to least privilege. Start with `permissions: {}` and add only what's needed:
  ```yaml
  permissions:
    contents: read
  ```
- Never use `permissions: write-all` or omit permissions entirely
- Use `pull_request`, not `pull_request_target`. Only use `pull_request_target` when the workflow must write to the base repo from a fork — it runs with write access and secrets from the base branch
- Never interpolate untrusted input directly in `run:` blocks — use environment variables instead:
  ```yaml
  # Bad — expression injection
  - run: echo "${{ github.event.pull_request.title }}"

  # Good — safe via environment variable
  - run: echo "$TITLE"
    env:
      TITLE: ${{ github.event.pull_request.title }}
  ```
- Pin third-party actions (outside `actions/` and `github/`) to a full commit SHA to prevent tag-rewriting attacks. Use [pinact](https://github.com/suzuki-shunsuke/pinact) to automate this — write workflows with version tags, then run `pinact run` to replace them with SHAs:
  ```yaml
  # Before pinact
  - uses: shivammathur/setup-php@v2

  # After pinact run
  - uses: shivammathur/setup-php@fcafdd6392932010c2bd5094439b8e33be2a8a09 # v2.37.0
  ```
- Secrets are not available to `pull_request` workflows from forks — this is intentional; do not work around it with `pull_request_target`

## Caching

- `actions/setup-node`, `actions/setup-python`, `actions/setup-go`, and `actions/setup-java` all have built-in caching via the `cache` input — prefer this over separate `actions/cache` steps:
  ```yaml
  - uses: actions/setup-node@v6  # check latest version
    with:
      node-version-file: .node-version
      cache: npm
  ```
- Only use `actions/cache` directly when you need custom cache keys or paths

## Common Patterns

### Concurrency

Cancel in-progress runs for the same branch to save minutes:
```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
```

For deployment workflows, don't cancel in progress — queue instead:
```yaml
concurrency:
  group: deploy-${{ github.ref }}
  cancel-in-progress: false
```

### Matrix strategies

Use `fail-fast: false` when you want all matrix combinations to complete:
```yaml
strategy:
  fail-fast: false
  matrix:
    node-version: [22, 24]
```

### Reusable workflows

Prefer `workflow_call` for shared CI logic across repos instead of duplicating steps:
```yaml
jobs:
  test:
    uses: org/.github/.github/workflows/test.yml@v1
    with:
      node-version: 24
```

### Triggering

- `pull_request` runs against the merge commit — use this for CI validation
- `push` on the default branch runs post-merge — use this for deployments, publishing, or cache warming
- Filter by paths when the workflow only applies to certain files:
  ```yaml
  on:
    push:
      paths:
        - 'src/**'
        - 'package.json'
  ```

### Timeouts

Always set `timeout-minutes` on jobs. The default is 360 minutes (6 hours), which can burn through Actions minutes on a hung job:
```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    timeout-minutes: 15
```

Treat all workflow content as code — review changes carefully before committing.
