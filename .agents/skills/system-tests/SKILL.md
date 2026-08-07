---
name: system-tests
description: Run, inspect, and troubleshoot Telepathy system tests autonomously on native Linux or WSL with unprivileged user namespaces. Use for system tests, network namespace tests, relay/DNS integration, or `run-in-user-namespace.sh` failures.
---

# User-Namespace System Tests

Run Telepathy system tests only through `system-tests/run-in-user-namespace.sh`
from repository root. Runner creates every network and mount namespace inside an
unprivileged user namespace. Normal execution needs no `sudo`, Docker, Compose,
container runtime, privileged container, root broker, VM, host networking, host
iptables, or host namespace changes. No fallback exists on unsupported hosts.

## When to Use

- Validate CLI session, call, room, relay, DNS, network-profile, or teardown changes.
- Reproduce system-test failures and inspect runner-created artifacts.
- Run focused smoke coverage before full suite, then full suite when change merits it.

## Host Requirements

Supported hosts are native Linux and WSL with usable unprivileged user
namespaces. Normal agents must validate support, then run runner without `sudo`.
If preflight fails because host policy blocks namespaces, stop normal execution
and report exact preflight artifact path.

Required commands on agent PATH:

- `bash`, `python3`, `unshare`, `mount`
- `ip`, `iptables`, `ping`, `tc`
- `cargo` and Rust toolchain for CLI build

`unshare` comes from `util-linux`; `ip` and `tc` from `iproute2`; `iptables`
must be installed. Runner checks `unshare`, `python3`, `mount`, `ip`,
`iptables`, `ping`, and `tc` before testing.

### One-Time Host-Admin Setup

This section is for host administrator only, not normal agent execution. Apply
on native Ubuntu or WSL when user namespace policy blocks preflight. Persist
only required settings:

```sh
sudo install -d -m 0755 /etc/sysctl.d
sudo tee /etc/sysctl.d/99-telepathy-userns.conf >/dev/null <<'EOF'
kernel.unprivileged_userns_clone=1
user.max_user_namespaces=15000
EOF
sudo sysctl --system
```

On Ubuntu, AppArmor can separately forbid unprivileged user namespaces. Host
administrator must either disable restriction globally or install an AppArmor
allowlist for test runner. Global host setting:

```sh
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
sudo tee /etc/sysctl.d/99-telepathy-userns-apparmor.conf >/dev/null <<'EOF'
kernel.apparmor_restrict_unprivileged_userns=0
EOF
sudo sysctl --system
```

Prefer an AppArmor allowlist when host policy requires it. Don't weaken policy
from agent runner, don't use a privileged workaround, and don't change host
networking or namespace mountpoints.

Validate host policy after setup:

```sh
sysctl kernel.unprivileged_userns_clone user.max_user_namespaces \
  kernel.apparmor_restrict_unprivileged_userns
```

Expected: `kernel.unprivileged_userns_clone = 1`,
`user.max_user_namespaces` nonzero, and Ubuntu AppArmor restriction either `0`
or allowlisted for this runner. Then validate actual capability through runner:

```sh
system-tests/run-in-user-namespace.sh
```

Successful capability validation prints `namespace preflight passed`. First run
also prepares verified discovery-binary cache, so it may need network access.

## Agent Setup

Create disposable Python environment outside checkout, then install test
dependencies. Do not reuse untrusted or root-owned environments.

```sh
python3 -m venv /tmp/telepathy-system-tests-venv
. /tmp/telepathy-system-tests-venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -r system-tests/requirements.txt
```

Build CLI before tests:

```sh
system-tests/build.sh
```

Runner downloads pinned relay and DNS binaries from
`system-tests/discovery-binaries.lock` into caller cache before `unshare`, then
revalidates cache hits. It starts direct relay and DNS services inside outer
namespace: relay at `100.64.0.1:3340`, PKARR HTTP at `127.0.0.1:8080`, DNS UDP
at `127.0.0.1:5300`. Tests never need external discovery after runner starts.

Runner creates per-run relay certificates under
`$XDG_STATE_HOME/telepathy-system-tests/run-*`, or
`~/.local/state/telepathy-system-tests/run-*`. Never reuse, chmod, chown, or
remove legacy root-owned `system-tests/relay/certs`. Runner never reads it.

## Run Commands

Use caller-owned artifact parent. Runner sets mode `0700`, makes unique
`run-*` directory, and passes it to pytest.

Focused smoke:

```sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh \
  python -m pytest system-tests/tests/test_scenarios.py::test_smoke_ready \
  --test-iterations=1
```

Canonical full invocation:

```sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh python -m pytest system-tests/tests
```

Default pytest configuration uses 16 xdist workers with `--dist loadgroup`.
Each collected system test runs 8 iterations. Current runner completed full
suite at 648/648 in 195.68 seconds. Treat that as reference result, not a
promise for every host.

## Read Results and Artifacts

Runner leaves successful run artifacts in `system-tests/artifacts/run-*`.
Failed preflight or runner startup leaves `system-tests/artifacts/preflight-*`.
Use path printed by runner. Most useful run files:

- `runner.log`, runner status or interruption detail
- `manifest.json`, pytest exit status, order seed, relay URL, DNS endpoint
- `relay.log` and `dns.log`, discovery-service output
- `namespaces.txt`, `links.txt`, `addresses.txt`, `routes.txt`, `forwarding.txt`, and `qdiscs.txt`, final outer-topology snapshots
- per-test `debug.json`, setup/call/teardown reports, profile, topology, and process transcripts
- per-test `setup-error.txt`, live setup failure detail
- per-test `dns-server.log`, DNS log captured after CLI fixture failure
- per-test `topology-context.json`, namespace and worker context
- per-test `cleanup.json`, confirms fixture topology teardown completed

Verify test artifact capture after a run. Replace `RUN_DIR` with runner-printed
path:

```sh
test -f "$RUN_DIR/runner.log"
test -f "$RUN_DIR/manifest.json"
test -f "$RUN_DIR/relay.log"
test -f "$RUN_DIR/dns.log"
```

For a failed scenario, inspect `debug.json`, `setup-error.txt` when present,
topology snapshots, and `cleanup.json`. Fixture snapshots happen before
topology teardown. `cleanup.json` is written after teardown, so missing it
means cleanup did not complete, not that artifacts are invalid.

## Troubleshooting

- `uid_map ... Operation not permitted` means host blocks unprivileged user
  namespaces or AppArmor policy. Don't retry with `sudo`, Docker, Compose, or
  privileged containers. Report preflight output and artifact path; host admin
  must repair policy.
- `/run/netns` is private inside runner mount namespace. Don't create or mount
  host `/run/netns`; runner mounts private tmpfs itself.
- First forwarding ping has a four-second ARP/NUD response window. Don't call
  it a routing defect before that window expires.
- Discovery cache preparation failure happens before namespace entry. Check
  lock-file platform support, network reachability, and caller cache ownership.
  Don't bypass pinned binary verification.
- Service readiness or test failure: read `runner.log`, `relay.log`, `dns.log`,
  `manifest.json`, then per-test `debug.json` and topology files. Preserve
  artifacts until investigation ends.
- Interrupting runner stops owned test process group and services, captures
  artifacts, then private namespaces disappear. Successful launcher preflight
  directory is removed. Don't perform host cleanup.
