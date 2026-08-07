---
title: Unprivileged User Namespace System Test Workflow
date: 2026-08-06
category: docs/solutions/conventions/
module: system test harness
problem_type: convention
component: user_namespace_runner
severity: high
applies_when:
  - Running Telepathy system tests that need namespaces, veth pairs, routing, forwarding, or network emulation
  - Investigating a system test failure that produced runner or topology artifacts
  - Preparing a Linux or WSL host for unprivileged system test execution
related_components:
  - system-tests/run-in-user-namespace.sh
  - system-tests/harness/namespace_runner.py
  - system-tests/harness/discovery.py
  - system-tests/harness/topology.py
  - docs/SYSTEM-TESTS.md
tags: [system-tests, user-namespaces, network-topology, discovery-cache, artifacts, forwarding]
---

# Unprivileged User Namespace System Test Workflow

## Context

Telepathy system tests need real Linux network behavior without changing host
network state or requiring privileged infrastructure. Run them only through
`system-tests/run-in-user-namespace.sh` on native Linux or WSL. The launcher
first prepares discovery binaries, then enters:

```sh
unshare --user --map-root-user --net --mount
```

The resulting namespace has mapped root only inside its own user namespace.
It does not grant host root access. The runner makes mounts private, creates a
private `/run/netns`, checks nested namespace and veth support, enables
namespace-local forwarding, verifies `iptables` forwarding, and verifies `tc
netem`. See `system-tests/run-in-user-namespace.sh` and
`system-tests/harness/namespace_runner.py`.

This remains a kernel-real topology. `system-tests/harness/topology.py` creates
client namespaces, veth pairs, routes, forwarding rules, and optional `tc
netem` profiles. It is not a mocked network.

## Guidance

Use one command path for every system-test run:

```sh
SYSTEM_TEST_ORDER_SEED=agent-full-suite-fixed \
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh python -m pytest system-tests/tests
```

Treat preflight as host support detection. A failed preflight means this host
cannot create required unprivileged namespaces or kernel topology primitives.
Repair user-namespace support or move to supported native Linux or WSL. Do not
substitute `sudo`, Docker, Compose, a privileged container, a Docker socket,
host networking, host iptables, a root broker, or a VM. No fallback path exists.

Discovery is staged before `unshare` removes host networking. The direct relay
and DNS binaries are locked to v1.0.2 in `system-tests/discovery-binaries.lock`.
`system-tests/harness/discovery.py` verifies archive and executable digests on
both download and cache hits. During a run, it revalidates cached binaries
without network access, then starts relay and DNS inside outer namespace.

Keep `SYSTEM_TEST_ARTIFACTS_DIR` set to a caller-owned location. Runner creates
mode `0700` run directories and records `runner.log`, relay and DNS logs,
namespace state, links, addresses, routes, forwarding rules, qdiscs, and
`manifest.json`. Manifest fields are `system_test_order_seed`,
`pytest_exit_status`, `relay_url`, `dns_endpoint`, and `ip_forward`. Per-run
relay certificates live under user state, outside artifact tree. Never use
checkout `system-tests/relay/certs` for a test run.

## Why This Matters

Unprivileged isolation protects host network state while preserving failures
that only real routing, forwarding, neighbor resolution, or traffic shaping can
show. It also keeps test evidence tied to exact topology state and test order.

Recorded full-suite evidence: `648 passed in 195.68s` with seed
`agent-full-suite-fixed`. That evidence belongs with runner manifest fields,
not with an unseeded local rerun.

The startup forwarding probe comes before network profiles. For two clients,
`TopologyManager` sends one forwarded `ping` with a four-second timeout before
applying `tc netem`. That allowance covers Linux NUD's three one-second ARP
solicitations under xdist contention. A transient first-packet delay is not a
reason to remove the probe, shorten its timeout, or replace topology with a
mock. See `system-tests/harness/topology.py` and
`system-tests/tests/test_topology.py`.

## When to Apply

- Before any system test command, especially on a fresh Linux or WSL host.
- When a test exercises relay discovery, direct peer routing, packet loss,
  latency, jitter, or multi-client rooms.
- When preflight reports missing namespace capabilities, `ip`, `iptables`,
  `ping`, `tc`, or mount support.
- When a test fails after topology setup, a relay or DNS readiness check, or a
  forwarded reachability probe.

## Examples

### Run preflight only

```sh
system-tests/run-in-user-namespace.sh
```

A success reports passed preflight. A failure is a supported-host problem, not
an invitation to retry with privileges.

### Inspect a failed run

Start with `runner.log`, `manifest.json`, `relay.log`, and `dns.log`. Then read
`namespaces.txt`, `links.txt`, `addresses.txt`, `routes.txt`, `forwarding.txt`,
and `qdiscs.txt`. For topology fixture failures, also inspect per-client
`*-neighbors.txt`, `*-routes.txt`, and `setup-error.txt` when present.

Some artifact files can report a failed snapshot command while still being
useful. Each snapshot stores command text, return code, stdout, and stderr, so
keep it as evidence instead of treating an incomplete snapshot as a second
failure. `system-tests/tests/test_scenarios.py` requires capture before one
topology teardown and writes `cleanup.json` after cleanup completes.

### Diagnose a forwarding-probe failure

Preserve live topology first. Inspect client neighbor tables for ARP or NUD
state, confirm each client default route points to its veth gateway, then check
outer `FORWARD` rules and interfaces. Inspect `qdiscs.txt` only after confirming
the failure happened before traffic shaping. Capture these files before teardown
removes namespaces and veth devices. The fixture deliberately captures live
state before cleanup so an ARP or forwarding fault remains observable.

Do not diagnose this by disabling the ping, deleting forwarding rules, or
changing topology into host networking. Those actions erase the condition the
system test is meant to prove.
