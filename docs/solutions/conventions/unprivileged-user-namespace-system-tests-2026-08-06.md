---
title: Hybrid Compose and User Namespace System Test Workflow
date: 2026-08-06
category: docs/solutions/conventions/
module: system test harness
problem_type: convention
component: user_namespace_runner
severity: high
applies_when:
  - Running Telepathy system tests that need namespaces, veth pairs, routing, forwarding, or network emulation
  - Choosing between local unprivileged and CI privileged system-test entrypoints
  - Investigating a system test failure that produced runner, Compose, Slirp, or topology artifacts
related_components:
  - system-tests/run-in-user-namespace.sh
  - system-tests/run-privileged.sh
  - system-tests/docker-compose.yml
  - system-tests/harness/namespace_runner.py
  - system-tests/harness/topology.py
  - docs/SYSTEM-TESTS.md
tags: [system-tests, docker-compose, user-namespaces, slirp4netns, network-topology, artifacts, forwarding]
---

# Hybrid Compose and User Namespace System Test Workflow

## Context

Telepathy system tests require real Linux networking. Docker Compose owns the
pinned v1.0.2 Iroh relay and DNS services in every environment. Local agents can
run without `sudo` through an unprivileged outer namespace, while CI runs pytest
with host root because user namespaces are not reliably available there.

The local launcher creates:

```sh
unshare --user --map-root-user --net --mount
```

It then waits for the child to report that UID mapping completed before attaching
`slirp4netns`. Slirp's host gateway, `192.0.2.2`, exposes host-network Compose
services to tests. Nested client traffic from `10.0.0.0/8` is masqueraded through
`tp-slirp0`.

## Guidance

Use the local path for agent development:

```sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh \
  python -m pytest system-tests/tests --save-artifacts failures
```

Use the privileged path in GitHub Actions or on an authorized disposable host:

```sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-privileged.sh \
  "$(command -v python)" -m pytest system-tests/tests --save-artifacts failures
```

Do not restore direct downloaded Iroh binaries. `system-tests/docker-compose.yml`
is the single service definition. `harness/discovery.py` contains readiness
probes only.

Docker socket access is still host-root-equivalent. The local entrypoint removes
the `sudo` requirement for namespace setup; it does not make Docker access safe
for untrusted code.

## Why This Matters

One Compose lifecycle avoids drift between local and CI service behavior. Slirp
preserves the user-namespace topology without host veth plumbing or root. The
privileged entrypoint keeps CI independent of hosted-runner user-namespace policy.
Both paths retain the failure evidence needed to diagnose real relay, DNS,
routing, forwarding, and netem behavior. The real local unprivileged suite
completed with `624 passed, 8 skipped in 210.07s`; skipped cases are privileged-
wrapper fake tests that do not apply as mapped root inside the user namespace.

## Artifact Pattern

Each run has a mode `0700` `run-*` directory. Start with `runner.log`,
`manifest.json`, `relay.log`, and `dns.log`. Local failures may also have
`slirp.log`. Inspect namespace, route, forwarding, and qdisc snapshots before
assuming a topology regression.

The forwarding probe still sends one ping before traffic profiles and allows four
seconds for ARP/NUD under xdist contention. Preserve that behavior; it catches
real forwarding failures rather than making tests easier to pass.

## Troubleshooting

- `uid_map ... Operation not permitted`: local user namespaces are blocked; fix
  host policy or use the privileged path where authorized.
- Slirp readiness failure: install `slirp4netns` and inspect `slirp.log`.
- Compose readiness failure: inspect `relay.log` and `dns.log`; check fixed ports
  and whether another run holds the system-test lock.
- Artifact permission failure in CI: verify `run-privileged.sh` restored artifact
  ownership to the invoking user before upload.
