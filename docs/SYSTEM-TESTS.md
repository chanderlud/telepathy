# Hybrid Compose System Tests

Telepathy system tests use Docker Compose to run pinned Iroh relay and DNS
services with host networking. They offer two execution paths:

- Local agents use `system-tests/run-in-user-namespace.sh`. This path needs no
  `sudo`; it creates an unprivileged outer user/network/mount namespace and uses
  `slirp4netns` to reach host Compose services at `192.0.2.2`.
- GitHub Actions invokes `system-tests/run-privileged.sh` as the runner user. The
  wrapper uses non-interactive `sudo` only for host topology and pytest because
  hosted runners may not permit unprivileged user namespace setup.

Both paths preserve nested client namespaces, veth pairs, forwarding rules,
reachability probes, and `tc netem`. Both generate per-run relay certificates
outside the checkout, capture Compose logs, and tear Compose down.

## Requirements

Common:

- Python 3.12 and `system-tests/requirements.txt`
- Rust toolchain for `system-tests/build.sh`
- Docker Engine with Compose v2 access
- `iproute2`, `iptables`, `iputils-ping`, `util-linux`, and `tc`

Local namespace mode additionally requires native Linux or WSL with unprivileged
user namespaces enabled, plus `slirp4netns`:

```sh
sudo apt-get update
sudo apt-get install slirp4netns
```

Docker socket access remains host-root-equivalent even though this path does not call
`sudo`.

Privileged mode additionally requires non-interactive `sudo`; invoke the wrapper as
the normal runner user so Compose and certificate generation remain caller-owned.

## Enable Unprivileged Namespaces

Only a host administrator should change these settings. On native Ubuntu or WSL,
persist basic user-namespace support:

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
accepts the global setting:

```sh
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
sudo tee /etc/sysctl.d/99-telepathy-userns-apparmor.conf >/dev/null <<'EOF'
kernel.apparmor_restrict_unprivileged_userns=0
EOF
sudo sysctl --system
```

Validate the host after installing `slirp4netns`:

```sh
sysctl kernel.unprivileged_userns_clone user.max_user_namespaces \
  kernel.apparmor_restrict_unprivileged_userns
system-tests/run-in-user-namespace.sh
```

Successful validation prints `namespace preflight passed`. If host policy cannot be
changed, use the privileged path only on an authorized disposable host or CI runner.

## Local Agent Run

From repository root:

```sh
python -m pip install -r system-tests/requirements.txt
bash system-tests/build.sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh python -m pytest \
  system-tests/tests --save-artifacts failures
```

The launcher starts Compose, creates a blocked outer namespace, and waits for the
child to report that `unshare --user --map-root-user --net --mount` has completed.
It then attaches `slirp4netns` with CIDR `192.0.2.0/24`, waits for Slirp's ready
file descriptor, and releases pytest. This handshake avoids racing UID-map setup.
Inside the namespace, host discovery is `192.0.2.2`: relay TCP/UDP `3340`, DNS UDP
`5300`, and PKARR HTTP `8080`. Nested `10.0.0.0/8` client traffic is masqueraded
through the Slirp TAP interface.

A fixed lock serializes runs because Compose services bind fixed host ports. A
second invocation fails clearly instead of stopping an active run.

## Privileged CI Run

From repository root:

```sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-privileged.sh \
  "$(command -v python)" -m pytest system-tests/tests --save-artifacts failures
```

This path does not create the outer user namespace or Slirp interface. Pytest runs
on the host as root; `TopologyManager` retains canonical relay address
`100.64.0.1` and per-client veth gateway DNS/PKARR endpoints. The wrapper enables
forwarding for the run, restores the prior sysctl value, removes the test loopback
alias, restores artifact ownership to the sudo caller, captures logs, and stops
Compose.

## Artifacts

Each entrypoint creates a mode `0700` `run-*` directory under
`SYSTEM_TEST_ARTIFACTS_DIR`. Important files include:

- `runner.log` for runner or pytest status
- `relay.log` and `dns.log` from Compose
- `slirp.log` in local namespace mode
- `manifest.json` with seed, pytest status, and discovery endpoints in local
  namespace mode
- namespace, link, address, route, forwarding, and qdisc snapshots
- per-test `debug.json`, `setup-error.txt`, `dns-server.log`, and `cleanup.json`

Failed local preflight retains the run directory and prints its path. Do not delete
artifacts until investigation ends.

## Current Validation

On this branch, the real local unprivileged path completed:

- smoke: `4 passed in 3.69s`
- full suite: `624 passed, 8 skipped in 210.07s`

The eight skips are the privileged-wrapper fake tests, which do not apply when the
suite itself is already running as mapped root inside the local user namespace.
Compose teardown was verified after the run. GitHub Actions then validated the
privileged entrypoint: all three PR sweeps passed, and full CI and Smoke reached
success.

## Troubleshooting

- `uid_map ... Operation not permitted`: the host blocks unprivileged user
  namespaces or AppArmor policy. Enable user namespaces, or use
  `run-privileged.sh` where root is authorized.
- `slirp4netns` missing or not ready: install it and inspect `slirp.log` in the
  printed run directory. Do not replace it with host networking in local mode.
- Compose startup failure: inspect `relay.log` and `dns.log`; check fixed ports
  `3340`, `3478`, `5300`, and `8080` and whether another run holds the lock.
- Namespace forwarding failure: inspect live topology snapshots before cleanup;
  preserve `cleanup.json` semantics.
