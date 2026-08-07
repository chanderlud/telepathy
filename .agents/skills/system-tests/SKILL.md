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
python -m pip install -r system-tests/requirements.txt
bash system-tests/build.sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh \
  python -m pytest system-tests/tests --save-artifacts failures
```

The launcher starts the Compose-pinned services, creates a blocked outer user
namespace, waits for the child to report that UID mapping completed, attaches
`slirp4netns`, waits for its ready signal, then runs pytest. Discovery endpoints
inside the namespace use Slirp host gateway `192.0.2.2`: relay `3340`, DNS `5300`,
and PKARR `8080`.

A fixed lock serializes runs because services bind fixed host ports. Do not bypass
it or run two stacks concurrently.

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

## Current Validation

Real local unprivileged run: `624 passed, 8 skipped in 210.07s`. The skips are
privileged-wrapper fake tests that do not apply as mapped root inside the local
user namespace. GitHub Actions validated the privileged entrypoint with all three
PR sweeps green; full CI and Smoke also passed.

## Troubleshooting

- `uid_map ... Operation not permitted`: enable unprivileged user namespaces or
  use the privileged entrypoint where root is authorized.
- Slirp startup failure: inspect the run's `slirp.log`; install `slirp4netns`.
- Compose failure: inspect `relay.log` and `dns.log`; check fixed ports and the
  system-test lock.
- Namespace or netem failure: inspect topology snapshots and `cleanup.json`; do
  not remove nested namespace process behavior or forwarding hardening.
