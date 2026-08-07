from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from harness.namespace_runner import (
    CommandError,
    NamespaceContext,
    PreflightError,
    _check_required_commands,
    configure_discovery_bridge,
    run_preflight,
)


class RecordingRunner:
    """Record commands and inject one command failure when requested."""

    def __init__(self, failing_prefix: tuple[str, ...] | None = None) -> None:
        self.commands: list[tuple[str, ...]] = []
        self.failing_prefix = failing_prefix

    def run(self, command: tuple[str, ...]) -> None:
        self.commands.append(command)
        if self.failing_prefix is not None and command[: len(self.failing_prefix)] == self.failing_prefix:
            raise CommandError(
                command=command,
                returncode=1,
                stdout="",
                stderr="operation not permitted",
            )


def supported_context(kernel_release: str = "6.8.0-generic") -> NamespaceContext:
    return NamespaceContext(
        host_uid=1000,
        effective_uid=0,
        uid_map="         0       1000          1\n",
        kernel_release=kernel_release,
        discovery_host="192.0.2.2",
        slirp_interface="tp-slirp0",
    )


def test_preflight_builds_private_nested_topology_then_disposes_it() -> None:
    runner = RecordingRunner()

    run_preflight(runner, supported_context())

    assert runner.commands[:4] == [
        ("mount", "--make-rprivate", "/"),
        (
            "mount",
            "-t",
            "tmpfs",
            "-o",
            "mode=0755,nosuid,nodev,noexec",
            "tmpfs",
            "/run",
        ),
        ("mkdir", "-p", "-m", "0755", "/run/netns"),
        (
            "mount",
            "-t",
            "tmpfs",
            "-o",
            "mode=0755,nosuid,nodev,noexec",
            "tmpfs",
            "/run/netns",
        ),
    ]
    assert ("sysctl", "-w", "net.ipv4.ip_forward=1") in runner.commands
    netns_add_index = runner.commands.index(
        ("ip", "netns", "add", "telepathy-preflight")
    )
    assert netns_add_index > 1
    assert (
        "ip",
        "netns",
        "exec",
        "telepathy-preflight",
        "ip",
        "route",
        "replace",
        "default",
        "via",
        "198.18.0.1",
    ) in runner.commands
    assert (
        "iptables",
        "-A",
        "FORWARD",
        "-i",
        "tp-preflight0",
        "-j",
        "ACCEPT",
    ) in runner.commands
    assert (
        "iptables",
        "-A",
        "FORWARD",
        "-o",
        "tp-preflight0",
        "-j",
        "ACCEPT",
    ) in runner.commands
    assert (
        "tc",
        "qdisc",
        "replace",
        "dev",
        "tp-preflight0",
        "root",
        "netem",
        "delay",
        "1ms",
    ) in runner.commands
    assert runner.commands[-5:] == [
        (
            "tc",
            "qdisc",
            "del",
            "dev",
            "tp-preflight0",
            "root",
        ),
        (
            "iptables",
            "-D",
            "FORWARD",
            "-o",
            "tp-preflight0",
            "-j",
            "ACCEPT",
        ),
        (
            "iptables",
            "-D",
            "FORWARD",
            "-i",
            "tp-preflight0",
            "-j",
            "ACCEPT",
        ),
        ("ip", "link", "del", "tp-preflight0"),
        ("ip", "netns", "del", "telepathy-preflight"),
    ]


def test_preflight_cleans_created_state_when_capability_probe_fails() -> None:
    runner = RecordingRunner(failing_prefix=("tc", "qdisc", "replace"))

    with pytest.raises(PreflightError, match="tc netem capability"):
        run_preflight(runner, supported_context())

    assert ("ip", "link", "del", "tp-preflight0") in runner.commands
    assert ("ip", "netns", "del", "telepathy-preflight") in runner.commands


def test_preflight_error_names_wsl_and_failed_command() -> None:
    runner = RecordingRunner(failing_prefix=("mount", "--make-rprivate"))

    with pytest.raises(PreflightError) as failure:
        run_preflight(runner, supported_context("5.15.153.1-microsoft-standard-WSL2"))

    message = str(failure.value)
    assert "WSL" in message
    assert "private mount propagation" in message
    assert "mount --make-rprivate /" in message
    assert "exit 1" in message
    assert "operation not permitted" in message


def test_preflight_rejects_unmapped_root_before_mount_commands() -> None:
    runner = RecordingRunner()
    context = NamespaceContext(
        host_uid=1000,
        effective_uid=1000,
        uid_map="      1000       1000          1\n",
        kernel_release="6.8.0-generic",
        discovery_host="192.0.2.2",
        slirp_interface="tp-slirp0",
    )

    with pytest.raises(PreflightError, match="mapped root identity"):
        run_preflight(runner, context)

    assert runner.commands == []


def test_preflight_creates_missing_netns_mountpoint_inside_private_namespace() -> None:
    runner = RecordingRunner()
    context = NamespaceContext(
        host_uid=1000,
        effective_uid=0,
        uid_map="         0       1000          1\n",
        kernel_release="6.8.0-generic",
        discovery_host="192.0.2.2",
        slirp_interface="tp-slirp0",
    )

    run_preflight(runner, context)

    mkdir_index = runner.commands.index(("mkdir", "-p", "-m", "0755", "/run/netns"))
    mount_index = runner.commands.index(
        (
            "mount",
            "-t",
            "tmpfs",
            "-o",
            "mode=0755,nosuid,nodev,noexec",
            "tmpfs",
            "/run/netns",
        )
    )
    assert mkdir_index < mount_index


def test_preflight_reports_each_missing_required_command(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def resolve_command(command: str) -> str | None:
        return None if command == "tc" else f"/usr/bin/{command}"

    monkeypatch.setattr("harness.namespace_runner.shutil.which", resolve_command)

    with pytest.raises(PreflightError, match="missing from PATH: tc"):
        _check_required_commands("native Linux")


def test_discovery_bridge_validates_slirp_and_masquerades_nested_clients() -> None:
    runner = RecordingRunner()

    configure_discovery_bridge(runner, supported_context())

    assert runner.commands == [
        ("ip", "link", "show", "dev", "tp-slirp0"),
        ("ip", "route", "get", "192.0.2.2"),
        (
            "iptables",
            "-t",
            "nat",
            "-A",
            "POSTROUTING",
            "-s",
            "10.0.0.0/8",
            "-o",
            "tp-slirp0",
            "-j",
            "MASQUERADE",
        ),
    ]


def _write_fake_command(path: Path, body: str) -> None:
    path.write_text(f"#!/usr/bin/env bash\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def test_shell_launcher_bridges_compose_through_slirp_and_preserves_seed(
    tmp_path: Path,
) -> None:
    docker_log = tmp_path / "docker.log"
    _write_fake_command(tmp_path / "cargo", "exit 0")
    _write_fake_command(
        tmp_path / "docker",
        "printf '%s\\n' \"$*\" >> \"${DOCKER_LOG}\"\n"
        "if [[ \"$*\" == *logs* ]]; then printf 'compose logs\\n'; fi",
    )
    _write_fake_command(
        tmp_path / "slirp4netns",
        "printf '1' >&3\n"
        "while true; do sleep 1; done",
    )
    _write_fake_command(
        tmp_path / "unshare",
        "while [[ \"${1:-}\" == --* ]]; do shift; done\n"
        "exec \"$@\"",
    )
    _write_fake_command(
        tmp_path / "python3",
        "printf 'discovery=%s\\n' \"${TELEPATHY_DISCOVERY_HOST}\"\n"
        "printf 'interface=%s\\n' \"${TELEPATHY_SLIRP_INTERFACE}\"\n"
        "printf 'seed=%s\\n' \"${SYSTEM_TEST_ORDER_SEED}\"\n"
        "printf 'artifacts=%s\\n' \"${SYSTEM_TEST_ARTIFACTS_DIR}\"\n"
        "printf 'args=%s\\n' \"$*\"",
    )
    launcher = Path(__file__).parents[1] / "run-in-user-namespace.sh"
    artifacts = tmp_path / "artifacts"
    environment = os.environ | {
        "PATH": f"{tmp_path}:{os.environ['PATH']}",
        "SYSTEM_TEST_ORDER_SEED": "seed-4821",
        "SYSTEM_TEST_ARTIFACTS_DIR": str(artifacts),
        "XDG_RUNTIME_DIR": str(tmp_path),
        "TMPDIR": str(tmp_path),
        "DOCKER_LOG": str(docker_log),
    }

    result = subprocess.run(
        [str(launcher), "python3", "-m", "pytest", "tests/test_topology.py"],
        cwd=launcher.parent,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines()[:3] == [
        "discovery=192.0.2.2",
        "interface=tp-slirp0",
        "seed=seed-4821",
    ]
    run_roots = list(artifacts.glob("run-*"))
    assert len(run_roots) == 1
    assert f"artifacts={run_roots[0]}" in result.stdout
    assert result.stdout.endswith(
        "namespace_runner.py -- python3 -m pytest tests/test_topology.py\n"
    )
    docker_commands = docker_log.read_text(encoding="utf-8")
    assert "up -d --wait" in docker_commands
    assert "logs --no-color iroh-relay" in docker_commands
    assert "logs --no-color iroh-dns-server" in docker_commands
    assert "down" in docker_commands
    assert (run_roots[0] / "relay.log").read_text(encoding="utf-8") == "compose logs\n"
    assert (run_roots[0] / "dns.log").read_text(encoding="utf-8") == "compose logs\n"


def test_given_unshare_denial_when_launcher_runs_then_it_keeps_diagnostic_artifact(
    tmp_path: Path,
) -> None:
    docker_log = tmp_path / "docker.log"
    _write_fake_command(tmp_path / "cargo", "exit 0")
    _write_fake_command(
        tmp_path / "docker", "printf '%s\\n' \"$*\" >> \"${DOCKER_LOG}\""
    )
    _write_fake_command(
        tmp_path / "slirp4netns",
        "printf '1' >&3\n"
        "while true; do sleep 1; done",
    )
    _write_fake_command(
        tmp_path / "unshare",
        "printf 'uid_map denied' >&2\n"
        "exit 1",
    )
    _write_fake_command(tmp_path / "python3", "exit 0")
    launcher = Path(__file__).parents[1] / "run-in-user-namespace.sh"
    artifacts = tmp_path / "artifacts"

    result = subprocess.run(
        [str(launcher), "python3", "-m", "pytest"],
        cwd=launcher.parent,
        env=os.environ
        | {
            "PATH": f"{tmp_path}:{os.environ['PATH']}",
            "SYSTEM_TEST_ARTIFACTS_DIR": str(artifacts),
            "XDG_RUNTIME_DIR": str(tmp_path),
            "TMPDIR": str(tmp_path),
            "DOCKER_LOG": str(docker_log),
        },
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )

    assert result.returncode == 2
    assert "uid_map denied" in result.stderr
    assert "namespace runner failed; artifacts:" in result.stderr
    run_roots = list(artifacts.glob("run-*"))
    assert len(run_roots) == 1
    assert "uid_map denied" in (run_roots[0] / "runner.log").read_text(
        encoding="utf-8"
    )
    assert "down" in docker_log.read_text(encoding="utf-8")


def test_privileged_launcher_keeps_compose_caller_owned_and_sudos_only_pytest(
    tmp_path: Path,
) -> None:
    if os.environ.get("TELEPATHY_DISCOVERY_HOST") is not None:
        pytest.skip("privileged wrapper appears as mapped root inside user namespace")

    docker_log = tmp_path / "docker.log"
    sudo_log = tmp_path / "sudo.log"
    _write_fake_command(tmp_path / "cargo", "exit 0")
    _write_fake_command(
        tmp_path / "docker",
        "printf '%s\\n' \"$*\" >> \"${DOCKER_LOG}\"\n"
        "if [[ \"$*\" == *logs* ]]; then printf 'compose logs\\n'; fi",
    )
    _write_fake_command(
        tmp_path / "sysctl",
        "if [[ \"$1\" == '-n' ]]; then printf '0\\n'; else printf '%s\\n' \"$*\" >> \"${SYSCTL_LOG}\"; fi",
    )
    _write_fake_command(
        tmp_path / "sudo",
        "printf '%s\\n' \"$*\" >> \"${SUDO_LOG}\"\n"
        "if [[ \"$*\" == '-n true' ]]; then exit 0; fi\n"
        "if [[ \"$1\" == '-E' ]]; then\n"
        "  shift\n"
        "  while [[ \"$1\" == *=* ]]; do shift; done\n"
        "  printf 'privileged-command=%s\\n' \"$*\"\n"
        "fi",
    )
    _write_fake_command(
        tmp_path / "python3",
        "if [[ \"$1\" == *wait-for-discovery.py ]]; then exit 0; fi\n"
        "printf 'unexpected-python=%s\\n' \"$*\"\n"
        "exit 1",
    )
    launcher = Path(__file__).parents[1] / "run-privileged.sh"
    artifacts = tmp_path / "artifacts"

    result = subprocess.run(
        [str(launcher), "python3", "-m", "pytest", "system-tests/tests"],
        cwd=launcher.parent,
        env=os.environ
        | {
            "PATH": f"{tmp_path}:{os.environ['PATH']}",
            "SYSTEM_TEST_ARTIFACTS_DIR": str(artifacts),
            "XDG_RUNTIME_DIR": str(tmp_path),
            "TMPDIR": str(tmp_path),
            "DOCKER_LOG": str(docker_log),
            "SUDO_LOG": str(sudo_log),
            "SYSCTL_LOG": str(tmp_path / "sysctl.log"),
        },
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == [
        "privileged-command=python3 -m pytest system-tests/tests"
    ]
    assert "system-test artifacts:" in result.stderr
    run_roots = list(artifacts.glob("run-*"))
    assert len(run_roots) == 1
    assert (run_roots[0] / "runner.log").read_text(
        encoding="utf-8"
    ) == "pytest exit status: 0\n"
    sudo_commands = sudo_log.read_text(encoding="utf-8")
    assert "-n true" in sudo_commands
    assert "sysctl -w net.ipv4.ip_forward=1" in sudo_commands
    assert "-E SYSTEM_TEST_ARTIFACTS_DIR=" in sudo_commands
    assert "chown -R" in sudo_commands
    docker_commands = docker_log.read_text(encoding="utf-8")
    assert "up -d --wait" in docker_commands
    assert "down" in docker_commands


def test_given_direct_runner_execution_when_no_launcher_identity_then_imports_and_fails_safely() -> None:
    runner = Path(__file__).parents[1] / "harness" / "namespace_runner.py"

    result = subprocess.run(
        ["python3", str(runner)],
        cwd=runner.parents[2],
        env={key: value for key, value in os.environ.items() if key != "TELEPATHY_HOST_UID"},
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )

    assert result.returncode == 2
    assert "TELEPATHY_HOST_UID is missing" in result.stderr
