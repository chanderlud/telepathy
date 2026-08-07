# User-Namespace System Tests

Run system tests only from native Linux or WSL through:

```sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh python -m pytest system-tests/tests
```

Runner enters `unshare --user --map-root-user --net --mount`, makes mount
propagation private, mounts private `/run/netns`, then proves nested namespaces,
veth pairs, routes, forwarding rules, and `tc netem`. Failed preflight means host
is unsupported. Repair host user-namespace support or use supported host.

Relay and DNS use pinned v1.0.2 binaries from `discovery-binaries.lock`. Runner
downloads only verified archives into user cache, revalidates cache hits, starts
both services inside outer namespace, and writes per-run certificates under user
state rather than checkout. Never use checkout `system-tests/relay/certs`.

Artifacts live under caller-owned `SYSTEM_TEST_ARTIFACTS_DIR` with mode `0700`.
Each run contains runner and discovery logs, topology state, `debug.json`, and
manifest data. Private certificate keys remain outside artifact tree.

No fallback exists: sudo, passwordless sudo, Docker, Compose, container runtime,
privileged container, Docker socket or group, host networking, host iptables,
host namespace mutation, root broker, and VM are prohibited.
