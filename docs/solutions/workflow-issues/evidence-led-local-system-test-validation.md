---
title: Evidence-Led Local System Test Validation and Race Triage
date: 2026-08-08
category: workflow-issues
module: system test validation
problem_type: workflow_issue
component: testing_framework
severity: high
applies_when:
  - "Validating Telepathy system tests locally across repeated runs"
  - "Triaging an unexpected system-test failure or runner interruption"
  - "Preparing concurrency-race evidence for a follow-up issue"
related_components:
  - "system-tests/run-in-user-namespace.sh"
  - "system-tests/build.sh"
  - "system-tests/conftest.py"
  - "system-tests/harness/namespace_runner.py"
  - "rust/telepathy-core/tests/core_integration_test.rs"
tags: [system-tests, race-triage, reproducibility, pytest, artifacts, user-namespaces]
---

# Evidence-Led Local System Test Validation and Race Triage

## Context

Repeated local system-test runs need evidence that separates product failures
from runner interruption. A ten-pass validation with serialized fixed-port runs
produced seven clean runs, two reproducible product failures, and one SIGINT
runner interruption. This is not a claim that every test passed.

The operational commands belong in
[the system-test skill](../../../.agents/skills/system-tests/SKILL.md). This
learning records how to make validation results reproducible and issue-ready.

## Guidance

Build before testing, then pass an explicit isolated virtualenv interpreter to
the user-namespace runner. Host `python` can point at an environment without
pytest. `system-tests/build.sh` produces binaries consumed by the suite, and
the runner launches exactly command passed to it.

Run fixed-port validations serially. The runner's lock protects shared service
ports, so concurrent stacks invalidate results. Give every pass both a unique
order seed and a unique artifact root. This validation used
`/tmp/telepathy-system-validation/pass-N` for artifact roots.

Keep artifacts for every non-clean outcome. The runner writes the order seed
and pytest exit status to `manifest.json`, while pytest stores the seed in
failed-test output and `debug.json`. Failure artifacts also capture topology
and cleanup state. Read this evidence before classifying outcome.

Treat SIGINT as runner interruption, not product-test evidence. The namespace
runner converts SIGINT into `RunnerInterrupted`, terminates its owned process
group, and restores signal handlers. A retained pytest failure after pytest's
own retries is product-test evidence instead.

For an unexpected pytest failure, first replay full collection with exact
`SYSTEM_TEST_ORDER_SEED`, then replay failing nodeid with same seed. Preserve
separate artifacts for both replays. Do not file a race report until failure
has an ignored real-path Rust integration repro that exercises same lifecycle
instead of a synthetic unit substitute.

## Why This Matters

Reproducible order plus preserved artifacts turn a result into evidence. Seed,
nodeid, exit status, topology state, and logs let later work distinguish
ordering-sensitive product behavior from host setup, runner lifecycle, or
manual interruption.

Ignored integration repros make race reports actionable. Two product failures
from this validation now have deterministic real-path Rust reproductions:
`caller_cancel_during_glare_allows_immediate_room_join_without_second_prompt`
and
`prompt_cancellation_then_manager_restart_promotes_replacement_and_connects`.
Their ignored attributes state current broken behavior, so they document known
regressions without making default integration runs fail.

## When to Apply

- Before treating repeated local system-test results as release or race-fix evidence.
- When fixed Compose ports, namespace setup, or test order could affect outcome.
- When a pytest failure needs a follow-up issue with a deterministic reproduction.
- When SIGINT, timeout, or teardown evidence might otherwise be mislabeled as product failure.

## Examples

Use isolated interpreter, build, serial seed, and distinct artifact root:

```sh
python3 -m venv /tmp/telepathy-system-tests-venv
/tmp/telepathy-system-tests-venv/bin/python -m pip install -r system-tests/requirements.txt
bash system-tests/build.sh

SYSTEM_TEST_ORDER_SEED="local-pass-1" \
SYSTEM_TEST_ARTIFACTS_DIR="/tmp/telepathy-system-validation/pass-1" \
  system-tests/run-in-user-namespace.sh \
  /tmp/telepathy-system-tests-venv/bin/python -m pytest system-tests/tests \
  --test-iterations 1 --save-artifacts failures
```

Replay full collection, then failing nodeid, with seed from failure evidence:

```sh
SYSTEM_TEST_ORDER_SEED='<seed-from-manifest>' \
SYSTEM_TEST_ARTIFACTS_DIR=/tmp/telepathy-system-validation/replay-seed \
  system-tests/run-in-user-namespace.sh \
  /tmp/telepathy-system-tests-venv/bin/python -m pytest system-tests/tests \
  --test-iterations 1 --save-artifacts failures

SYSTEM_TEST_ORDER_SEED='<seed-from-manifest>' \
SYSTEM_TEST_ARTIFACTS_DIR=/tmp/telepathy-system-validation/replay-nodeid \
  system-tests/run-in-user-namespace.sh \
  /tmp/telepathy-system-tests-venv/bin/python -m pytest '<failing-nodeid>' \
  --test-iterations 1 --save-artifacts all
```

## Related

- [Hybrid Compose and User Namespace System Test Workflow](../conventions/unprivileged-user-namespace-system-tests-2026-08-06.md) covers environment topology and runner modes.
- [Issue #91](https://github.com/chanderlud/telepathy/issues/91) and [issue #92](https://github.com/chanderlud/telepathy/issues/92) track follow-up product failures from this validation.
- `system-tests/conftest.py:138-174` records each failed nodeid and its order seed.
- `system-tests/harness/namespace_runner.py:427-463` records manifest evidence and classifies SIGINT as runner interruption.
- `rust/telepathy-core/tests/core_integration_test/call_lifecycle.rs:2691-2747` and `rust/telepathy-core/tests/core_integration_test/session_lifecycle.rs:271-277` contain ignored real-path reproductions.
