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


def test_shell_launcher_execs_exact_outer_namespace_command_and_preserves_seed(
    tmp_path: Path,
) -> None:
    fake_unshare = tmp_path / "unshare"
    fake_python = tmp_path / "python3"
    fake_python.write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
    fake_python.chmod(0o755)
    fake_unshare.write_text(
        "#!/usr/bin/env bash\n"
        "printf 'seed=%s\\n' \"${SYSTEM_TEST_ORDER_SEED-}\"\n"
        "printf '%s\\n' \"$@\"\n",
        encoding="utf-8",
    )
    fake_unshare.chmod(0o755)
    launcher = Path(__file__).parents[1] / "run-in-user-namespace.sh"
    environment = os.environ | {
        "PATH": f"{tmp_path}:{os.environ['PATH']}",
        "SYSTEM_TEST_ORDER_SEED": "seed-4821",
    }

    result = subprocess.run(
        [str(launcher), "python3", "-m", "pytest", "tests/test_topology.py"],
        cwd=launcher.parent,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )

    assert result.returncode == 0
    lines = result.stdout.splitlines()
    assert lines[:5] == [
        "seed=seed-4821",
        "--user",
        "--map-root-user",
        "--net",
        "--mount",
    ]
    assert lines[5] == "python3"
    assert Path(lines[6]).name == "namespace_runner.py"
    assert lines[-5:] == [
        "--",
        "python3",
        "-m",
        "pytest",
        "tests/test_topology.py",
    ]


def test_given_unshare_denial_when_launcher_runs_then_it_keeps_diagnostic_artifact(
    tmp_path: Path,
) -> None:
    fake_unshare = tmp_path / "unshare"
    fake_python = tmp_path / "python3"
    fake_python.write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
    fake_python.chmod(0o755)
    fake_unshare.write_text(
        "#!/usr/bin/env bash\n"
        "printf 'uid_map denied' >&2\n"
        "exit 1\n",
        encoding="utf-8",
    )
    fake_unshare.chmod(0o755)
    launcher = Path(__file__).parents[1] / "run-in-user-namespace.sh"
    artifacts = tmp_path / "artifacts"

    result = subprocess.run(
        [str(launcher), "python3", "-m", "pytest"],
        cwd=launcher.parent,
        env=os.environ
        | {"PATH": f"{tmp_path}:{os.environ['PATH']}", "SYSTEM_TEST_ARTIFACTS_DIR": str(artifacts)},
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )

    assert result.returncode == 1
    assert "uid_map denied" in result.stderr
    assert "namespace runner failed; artifacts:" in result.stderr
    logs = list(artifacts.glob("preflight-*/runner.log"))
    assert len(logs) == 1
    assert logs[0].read_text(encoding="utf-8") == "uid_map denied"


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
