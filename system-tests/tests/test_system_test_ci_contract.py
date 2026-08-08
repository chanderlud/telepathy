from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).parents[2]


def test_given_system_workflow_when_read_then_it_uses_privileged_compose_entrypoint() -> None:
    workflow = (ROOT / ".github" / "workflows" / "system-tests.yml").read_text(
        encoding="utf-8"
    )

    assert 'PYTHON_BIN="$(command -v python)"' in workflow
    assert 'system-tests/run-privileged.sh "$PYTHON_BIN" -m pytest' in workflow
    assert "run-in-user-namespace.sh" not in workflow
    assert "libasound2-dev" in workflow
    assert "iproute2" in workflow
    assert "iptables" in workflow
    assert "iputils-ping" in workflow
    assert "timeout-minutes: 40" in workflow
    assert "fail-fast: false" in workflow
    assert "actions/upload-artifact@v7" in workflow


def test_given_sweep_callers_when_read_then_pr_uses_three_and_nightly_uses_ten() -> None:
    ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    nightly = (ROOT / ".github" / "workflows" / "system-tests-nightly.yml").read_text(
        encoding="utf-8"
    )

    assert "sweep_indices: '[0,1,2]'" in ci
    assert "sweep_indices: '[0,1,2,3,4,5,6,7,8,9]'" in nightly


def test_given_hybrid_system_tests_when_read_then_compose_and_entrypoints_exist() -> None:
    system_tests = ROOT / "system-tests"
    compose = (system_tests / "docker-compose.yml").read_text(encoding="utf-8")
    up = (system_tests / "up.sh").read_text(encoding="utf-8")
    down = (system_tests / "down.sh").read_text(encoding="utf-8")
    local = (system_tests / "run-in-user-namespace.sh").read_text(encoding="utf-8")
    privileged = (system_tests / "run-privileged.sh").read_text(encoding="utf-8")

    assert "n0computer/iroh-relay:v1.0.2" in compose
    assert "n0computer/iroh-dns-server:v1.0.2" in compose
    assert compose.count("network_mode: host") == 2
    assert "TELEPATHY_RELAY_CERTS" in compose
    assert "gen-certs.sh" in up
    assert "up -d --wait" in up
    assert "down" in down
    assert "slirp4netns" in local
    assert "192.0.2.2" in local
    assert "run-in-user-namespace.sh" not in privileged
    assert '[[ "${EUID}" -eq 0 ]]' in privileged
    assert "sudo -n true" in privileged
    assert "sudo -E" in privileged
    assert "capture-discovery-logs.sh" in privileged
    assert "wait-for-discovery.py" in privileged
    assert "chown -R" in privileged


def test_given_direct_binary_discovery_when_migrated_then_lock_and_downloads_are_removed() -> None:
    system_tests = ROOT / "system-tests"

    assert not (system_tests / "discovery-binaries.lock").exists()
    discovery = (system_tests / "harness" / "discovery.py").read_text(encoding="utf-8")
    assert "urllib.request" not in discovery
    assert "tarfile" not in discovery
    assert "DiscoveryServices" not in discovery
