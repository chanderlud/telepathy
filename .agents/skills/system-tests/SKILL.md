---
name: system-tests
description: Run, inspect, and troubleshoot Telepathy hybrid Docker Compose system tests through unprivileged local namespaces or privileged CI. Use for relay/DNS integration, nested client namespaces, Slirp, network profiles, or teardown failures.
---

# Hybrid Compose System Tests

Run Iroh relay and DNS through Docker Compose. Local agents use
`system-tests/run-in-user-namespace.sh`; GitHub Actions uses
`system-tests/run-privileged.sh`. Both paths preserve nested client
namespaces, veth routing, forwarding rules, reachability checks, and `tc netem`.

## Requirements

- Docker Engine with Compose v2 access
- Linux or WSL with unprivileged user namespaces for local mode
- `slirp4netns` for local mode (`sudo apt-get install slirp4netns`)
- `iproute2`, `iptables`, `iputils-ping`, `util-linux`, and `tc`
- Python 3.12 plus `system-tests/requirements.txt`
- Rust toolchain for `system-tests/build.sh`
- Non-interactive `sudo` only for the privileged CI path

Docker socket access is host-root-equivalent. The local path avoids `sudo`, but it
does not make Docker access unprivileged.

## Enable Unprivileged Namespaces

Only a host administrator should apply these settings. Agents may run them when the
user asks for environment setup, but must not silently weaken host policy.

On native Ubuntu or WSL, persist basic user-namespace support:

```sh
sudo install -d -m 0755 /etc/sysctl.d
sudo tee /etc/sysctl.d/99-telepathy-userns.conf >/dev/null <<'EOF'
kernel.unprivileged_userns_clone=1
user.max_user_namespaces=15000
EOF
sudo sysctl --system
```

Ubuntu may also restrict unprivileged user namespaces through AppArmor. Prefer an
AppArmor allowlist for this runner when policy requires one. If the administrator
accepts the global setting, use:

```sh
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
sudo tee /etc/sysctl.d/99-telepathy-userns-apparmor.conf >/dev/null <<'EOF'
kernel.apparmor_restrict_unprivileged_userns=0
EOF
sudo sysctl --system
```

Install Slirp and validate host policy:

```sh
sudo apt-get update
sudo apt-get install slirp4netns
sysctl kernel.unprivileged_userns_clone user.max_user_namespaces \
  kernel.apparmor_restrict_unprivileged_userns
system-tests/run-in-user-namespace.sh
```

Successful validation prints `namespace preflight passed`. A failure means the host
does not support the unprivileged path; keep the printed artifact directory.

## Privileged Option

Use `system-tests/run-privileged.sh` only in GitHub Actions or on another
authorized, disposable host where non-interactive `sudo` is available. Invoke it as
the normal runner user; the wrapper starts Compose and generates certificates as
that user, then uses `sudo` only for host topology and pytest. It restores
forwarding state, removes the test loopback alias, tears Compose down, and repairs
artifact ownership. Do not use it as a sandbox for untrusted code.

## Local Agent Run

From repository root:

```sh
python3 -m venv --upgrade /tmp/telepathy-system-tests-venv
/tmp/telepathy-system-tests-venv/bin/python -m pip install -r system-tests/requirements.txt
/tmp/telepathy-system-tests-venv/bin/python -m pytest --version
/tmp/telepathy-system-tests-venv/bin/python -m pytest system-tests/tests/test_scenario.py
bash system-tests/build.sh
SYSTEM_TEST_ARTIFACTS_DIR=/tmp/telepathy-system-tests-artifacts/local \
  system-tests/run-in-user-namespace.sh \
  /tmp/telepathy-system-tests-venv/bin/python -m pytest system-tests/tests \
  --test-iterations 1 --save-artifacts failures
```

Do not activate this virtualenv. `/usr/bin/python3` may not provide pytest.
Run every local pytest command through
`/tmp/telepathy-system-tests-venv/bin/python`, which currently reports
`pytest 9.1.1` with `-m pytest --version`. The user-namespace runner executes
the explicit interpreter passed to it, so retain that path for Compose-backed
runs. Keep this isolated virtualenv outside the repository.

The launcher starts the Compose-pinned services, creates a blocked outer user
namespace, waits for the child to report that UID mapping completed, attaches
`slirp4netns`, waits for its ready signal, then runs pytest. Discovery endpoints
inside the namespace use Slirp host gateway `192.0.2.2`: relay `3340`, DNS `5300`,
and PKARR `8080`.

A fixed lock serializes runs because services bind fixed host ports. Do not bypass
it or run two stacks concurrently.

For particularly complex, risky, or concurrency-sensitive changes, an optional
ten-pass validation can expose order-dependent failures. Run passes serially.
Each pass has one test iteration, an explicit order seed, and a unique artifact
root:

```sh
status=0
for pass in $(seq 1 10); do
  SYSTEM_TEST_ORDER_SEED="local-pass-${pass}" \
  SYSTEM_TEST_ARTIFACTS_DIR="/tmp/telepathy-system-tests-artifacts/pass-${pass}" \
    system-tests/run-in-user-namespace.sh \
    /tmp/telepathy-system-tests-venv/bin/python -m pytest system-tests/tests \
    --test-iterations 1 --save-artifacts failures || status=1
done
exit "${status}"
```

Lock-required serialization protects fixed host ports. Do not parallelize this
loop or bypass the runner lock.

## Privileged CI Run

```sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-privileged.sh \
  "$(command -v python)" -m pytest system-tests/tests --save-artifacts failures
```

The privileged wrapper starts as the calling user, keeps Compose and certificate
state caller-owned, then uses sudo for pytest and host topology. It restores the
previous sysctl value, removes the canonical relay loopback alias, captures Compose
logs, stops Compose, and chowns artifacts back to the caller.

## Artifacts

Each run creates a mode `0700` `run-*` directory containing:

- `runner.log`, `relay.log`, `dns.log`, and local-mode `slirp.log`
- local-mode `manifest.json`
- namespace, link, address, route, forwarding, and qdisc snapshots
- per-test `debug.json`, `setup-error.txt`, `dns-server.log`, and `cleanup.json`

Use artifacts to classify failures. Read `manifest.json` for order seed and pytest
exit status, then inspect `runner.log`, service logs, and per-test `debug.json`.
For retained failure artifacts needing diagnosis, use the
[system-test artifact analysis skill](../system-test-artifact-analysis/SKILL.md).
A retained pytest failure after its built-in retries is product-test evidence.
First rerun complete collection with exact `SYSTEM_TEST_ORDER_SEED`, then replay
the failing nodeid with that same seed. For a parameterized `iter-N` nodeid, set
`<replay-iterations>` to at least `N + 1` in both commands so pytest collects
that parameter. Use `1` for `iter-0`. A SIGINT or runner interruption is
infrastructure evidence, not a product issue. Record skips explicitly alongside
pass, failure, and interruption outcomes.

```sh
SYSTEM_TEST_ORDER_SEED='<seed-from-manifest>' \
SYSTEM_TEST_ARTIFACTS_DIR=/tmp/telepathy-system-tests-artifacts/replay-seed \
  system-tests/run-in-user-namespace.sh \
  /tmp/telepathy-system-tests-venv/bin/python -m pytest system-tests/tests \
  --test-iterations '<replay-iterations>' --save-artifacts failures

SYSTEM_TEST_ORDER_SEED='<seed-from-manifest>' \
SYSTEM_TEST_ARTIFACTS_DIR=/tmp/telepathy-system-tests-artifacts/replay-nodeid \
  system-tests/run-in-user-namespace.sh \
  /tmp/telepathy-system-tests-venv/bin/python -m pytest '<failing-nodeid>' \
  --test-iterations '<replay-iterations>' --save-artifacts all
```

## Validation Guidance

Regular local validation is one run with one test iteration and retained failure
artifacts. For optional ten-pass validation, list each seed, artifact root, pass
or failure result, skips, and interruptions. Keep retained-failure evidence with
its manifest, logs, and `debug.json`; do not infer product status from a runner
interruption.

## Troubleshooting

- `uid_map ... Operation not permitted`: enable unprivileged user namespaces or
  use the privileged entrypoint where root is authorized.
- Slirp startup failure: inspect the run's `slirp.log`; install `slirp4netns`.
- Compose failure: inspect `relay.log` and `dns.log`; check fixed ports and the
  system-test lock.
- Namespace or netem failure: inspect topology snapshots and `cleanup.json`; do
  not remove nested namespace process behavior or forwarding hardening.
