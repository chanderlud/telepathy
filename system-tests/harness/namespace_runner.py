from __future__ import annotations

# noqa: SIZE_OK - fixed capability and cleanup command table is one state machine.

import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Final, Protocol

_SYSTEM_TEST_ROOT: Final = Path(__file__).resolve().parents[1]
if str(_SYSTEM_TEST_ROOT) not in sys.path:
    sys.path.insert(0, str(_SYSTEM_TEST_ROOT))

from harness.discovery import (
    DnsUdpReadinessProbe,
    HttpReadinessProbe,
    ReadinessTimeoutError,
    wait_for_readiness,
)

_COMMAND_TIMEOUT_SECONDS: Final = 10.0
_NETNS_NAME: Final = "telepathy-preflight"
_ROOT_INTERFACE: Final = "tp-preflight0"
_PEER_INTERFACE: Final = "tp-preflight1"
_REQUIRED_COMMANDS: Final = ("mount", "ip", "iptables", "ping", "tc")
_RELAY_ADDRESS: Final = "100.64.0.1"
_DISCOVERY_READY_TIMEOUT_SECONDS: Final = 30.0
_DISCOVERY_HOST_ENV: Final = "TELEPATHY_DISCOVERY_HOST"
_SLIRP_INTERFACE_ENV: Final = "TELEPATHY_SLIRP_INTERFACE"
_RUN_ARTIFACT_ENV: Final = "TELEPATHY_RUN_ARTIFACT_DIR"


class RunnerInterrupted(Exception):
    """Raised by owned signal handler so runner cleanup can complete."""


@dataclass(frozen=True, slots=True)
class CommandError(Exception):
    command: tuple[str, ...]
    returncode: int | None
    stdout: str
    stderr: str

    def __str__(self) -> str:
        status = "timed out" if self.returncode is None else f"exit {self.returncode}"
        detail = self.stderr.strip() or self.stdout.strip() or "no command output"
        return f"{' '.join(self.command)} ({status}): {detail}"


@dataclass(frozen=True, slots=True)
class PreflightError(Exception):
    environment: str
    prerequisite: str
    detail: str

    def __str__(self) -> str:
        return (
            f"unsupported {self.environment}: {self.prerequisite} failed\n"
            f"{self.detail}\n"
            "Enable unprivileged user namespaces and Slirp on this host, or use "
            "system-tests/run-privileged.sh where host root is available."
        )


@dataclass(frozen=True, slots=True)
class NamespaceContext:
    host_uid: int
    effective_uid: int
    uid_map: str
    kernel_release: str
    discovery_host: str
    slirp_interface: str

    @property
    def environment(self) -> str:
        return "WSL" if "microsoft" in self.kernel_release.casefold() else "native Linux"


@dataclass(frozen=True, slots=True)
class _ProbeStep:
    prerequisite: str
    command: tuple[str, ...]
    cleanup: tuple[str, ...] | None = None


class CommandRunner(Protocol):
    def run(self, command: tuple[str, ...]) -> None: ...


class SubprocessCommandRunner:
    def run(self, command: tuple[str, ...]) -> None:
        try:
            result = subprocess.run(
                command,
                capture_output=True,
                text=True,
                check=False,
                timeout=_COMMAND_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired as error:
            stdout = error.stdout if isinstance(error.stdout, str) else ""
            stderr = error.stderr if isinstance(error.stderr, str) else ""
            raise CommandError(command, None, stdout, stderr) from error
        except OSError as error:
            raise CommandError(command, None, "", str(error)) from error

        if result.returncode != 0:
            raise CommandError(
                command=command,
                returncode=result.returncode,
                stdout=result.stdout,
                stderr=result.stderr,
            )


def _require(
    runner: CommandRunner,
    context: NamespaceContext,
    step: _ProbeStep,
) -> None:
    try:
        runner.run(step.command)
    except CommandError as error:
        raise PreflightError(
            context.environment, step.prerequisite, str(error)
        ) from error


def _validate_context(context: NamespaceContext) -> None:
    expected_mapping = (0, context.host_uid, 1)
    try:
        mappings = [
            tuple(int(field) for field in line.split())
            for line in context.uid_map.splitlines()
        ]
    except ValueError as error:
        raise PreflightError(
            context.environment,
            "mapped root identity",
            f"invalid /proc/self/uid_map: {context.uid_map!r}",
        ) from error

    if context.effective_uid != 0 or expected_mapping not in mappings:
        raise PreflightError(
            context.environment,
            "mapped root identity",
            f"expected effective uid 0 mapped to host uid {context.host_uid}; "
            f"effective uid is {context.effective_uid}, uid_map is {context.uid_map!r}",
        )


def _cleanup_probe(
    runner: CommandRunner,
    context: NamespaceContext,
    commands: list[tuple[str, ...]],
) -> None:
    first_error: CommandError | None = None
    for command in reversed(commands):
        try:
            runner.run(command)
        except CommandError as error:
            if first_error is None:
                first_error = error
    if first_error is not None and sys.exception() is None:
        raise PreflightError(
            context.environment,
            "nested topology cleanup",
            str(first_error),
        ) from first_error


def _probe_nested_topology(runner: CommandRunner, context: NamespaceContext) -> None:
    netns = ("ip", "netns", "exec", _NETNS_NAME)
    forward_input = ("iptables", "-A", "FORWARD", "-i", _ROOT_INTERFACE, "-j", "ACCEPT")
    forward_output = ("iptables", "-A", "FORWARD", "-o", _ROOT_INTERFACE, "-j", "ACCEPT")
    steps = (
        _ProbeStep(
            "nested network namespace capability",
            ("ip", "netns", "add", _NETNS_NAME),
            ("ip", "netns", "del", _NETNS_NAME),
        ),
        _ProbeStep(
            "veth capability",
            ("ip", "link", "add", _ROOT_INTERFACE, "type", "veth", "peer", "name", _PEER_INTERFACE),
            ("ip", "link", "del", _ROOT_INTERFACE),
        ),
        _ProbeStep(
            "nested veth capability",
            ("ip", "link", "set", _PEER_INTERFACE, "netns", _NETNS_NAME),
        ),
        _ProbeStep(
            "veth address capability",
            ("ip", "addr", "add", "198.18.0.1/30", "dev", _ROOT_INTERFACE),
        ),
        _ProbeStep("veth link capability", ("ip", "link", "set", _ROOT_INTERFACE, "up")),
        _ProbeStep("nested route capability", (*netns, "ip", "link", "set", "lo", "up")),
        _ProbeStep(
            "nested route capability",
            (*netns, "ip", "addr", "add", "198.18.0.2/30", "dev", _PEER_INTERFACE),
        ),
        _ProbeStep("nested route capability", (*netns, "ip", "link", "set", _PEER_INTERFACE, "up")),
        _ProbeStep(
            "nested route capability",
            (*netns, "ip", "route", "replace", "default", "via", "198.18.0.1"),
        ),
        _ProbeStep(
            "iptables forwarding capability", forward_input, ("iptables", "-D", *forward_input[2:])
        ),
        _ProbeStep(
            "iptables forwarding capability", forward_output, ("iptables", "-D", *forward_output[2:])
        ),
        _ProbeStep(
            "tc netem capability",
            ("tc", "qdisc", "replace", "dev", _ROOT_INTERFACE, "root", "netem", "delay", "1ms"),
            ("tc", "qdisc", "del", "dev", _ROOT_INTERFACE, "root"),
        ),
    )
    cleanup: list[tuple[str, ...]] = []
    try:
        for step in steps:
            _require(runner, context, step)
            if step.cleanup is not None:
                cleanup.append(step.cleanup)
    finally:
        _cleanup_probe(runner, context, cleanup)


def run_preflight(runner: CommandRunner, context: NamespaceContext) -> None:
    _validate_context(context)
    _require(
        runner,
        context,
        _ProbeStep(
            "private mount propagation", ("mount", "--make-rprivate", "/")
        ),
    )
    _require(
        runner,
        context,
        _ProbeStep(
            "private /run mount",
            (
                "mount",
                "-t",
                "tmpfs",
                "-o",
                "mode=0755,nosuid,nodev,noexec",
                "tmpfs",
                "/run",
            ),
        ),
    )
    _require(
        runner,
        context,
        _ProbeStep(
            "private /run/netns directory",
            ("mkdir", "-p", "-m", "0755", "/run/netns"),
        ),
    )
    _require(
        runner,
        context,
        _ProbeStep(
            "private /run/netns mount",
            (
                "mount",
                "-t",
                "tmpfs",
                "-o",
                "mode=0755,nosuid,nodev,noexec",
                "tmpfs",
                "/run/netns",
            ),
        ),
    )
    _require(
        runner,
        context,
        _ProbeStep(
            "outer loopback capability", ("ip", "link", "set", "lo", "up")
        ),
    )
    _require(
        runner,
        context,
        _ProbeStep(
            "outer forwarding capability", ("sysctl", "-w", "net.ipv4.ip_forward=1")
        ),
    )
    _probe_nested_topology(runner, context)


def configure_discovery_bridge(runner: CommandRunner, context: NamespaceContext) -> None:
    """Route nested test clients through Slirp to host-network Compose services."""
    _require(
        runner,
        context,
        _ProbeStep(
            "Slirp interface",
            ("ip", "link", "show", "dev", context.slirp_interface),
        ),
    )
    _require(
        runner,
        context,
        _ProbeStep(
            "Slirp host route",
            ("ip", "route", "get", context.discovery_host),
        ),
    )
    _require(
        runner,
        context,
        _ProbeStep(
            "Slirp masquerade",
            (
                "iptables",
                "-t",
                "nat",
                "-A",
                "POSTROUTING",
                "-s",
                "10.0.0.0/8",
                "-o",
                context.slirp_interface,
                "-j",
                "MASQUERADE",
            ),
        ),
    )


def _read_context() -> NamespaceContext:
    raw_host_uid = os.environ.get("TELEPATHY_HOST_UID")
    if raw_host_uid is None:
        raise PreflightError(
            "Linux",
            "launcher identity",
            "TELEPATHY_HOST_UID is missing; invoke run-in-user-namespace.sh",
        )
    try:
        host_uid = int(raw_host_uid)
    except ValueError as error:
        raise PreflightError(
            "Linux",
            "launcher identity",
            f"TELEPATHY_HOST_UID is not an integer: {raw_host_uid!r}",
        ) from error
    discovery_host = os.environ.get(_DISCOVERY_HOST_ENV)
    slirp_interface = os.environ.get(_SLIRP_INTERFACE_ENV)
    if discovery_host is None or slirp_interface is None:
        raise PreflightError(
            "Linux",
            "Slirp discovery bridge",
            f"{_DISCOVERY_HOST_ENV} and {_SLIRP_INTERFACE_ENV} are required",
        )
    return NamespaceContext(
        host_uid=host_uid,
        effective_uid=os.geteuid(),
        uid_map=Path("/proc/self/uid_map").read_text(encoding="utf-8"),
        kernel_release=platform.release(),
        discovery_host=discovery_host,
        slirp_interface=slirp_interface,
    )


def _check_required_commands(environment: str) -> None:
    missing = [command for command in _REQUIRED_COMMANDS if shutil.which(command) is None]
    if missing:
        raise PreflightError(
            environment,
            "required commands",
            f"missing from PATH: {', '.join(missing)}; install util-linux, iproute2, and iptables",
        )


def _private_run_directory(parent: Path, prefix: str) -> Path:
    parent.mkdir(parents=True, exist_ok=True)
    parent.chmod(0o700)
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
    path.chmod(0o700)
    return path


def _artifact_root() -> Path:
    explicit = os.environ.get(_RUN_ARTIFACT_ENV)
    if explicit is not None:
        path = Path(explicit).resolve()
        path.mkdir(parents=True, exist_ok=True)
        path.chmod(0o700)
        return path
    artifact_parent = Path(
        os.environ.get("SYSTEM_TEST_ARTIFACTS_DIR", _SYSTEM_TEST_ROOT / "artifacts")
    ).resolve()
    return _private_run_directory(artifact_parent, "run-")


def _capture_command(path: Path, command: tuple[str, ...]) -> None:
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    path.write_text(
        result.stdout + ("\n--- stderr ---\n" + result.stderr if result.stderr else ""),
        encoding="utf-8",
    )


def _capture_artifacts(
    artifact_root: Path, context: NamespaceContext, status: int | None
) -> None:
    commands = {
        "namespaces.txt": ("ip", "netns", "list"),
        "links.txt": ("ip", "-details", "link", "show"),
        "addresses.txt": ("ip", "addr", "show"),
        "routes.txt": ("ip", "route", "show", "table", "all"),
        "forwarding.txt": ("iptables", "-S", "FORWARD"),
        "qdiscs.txt": ("tc", "qdisc", "show"),
    }
    for name, command in commands.items():
        _capture_command(artifact_root / name, command)
    (artifact_root / "manifest.json").write_text(
        json.dumps(
            {
                "system_test_order_seed": os.environ.get("SYSTEM_TEST_ORDER_SEED"),
                "pytest_exit_status": status,
                "relay_url": f"http://{context.discovery_host}:3340",
                "dns_endpoint": f"{context.discovery_host}:5300",
                "ip_forward": "enabled",
            },
            indent=2,
        ),
        encoding="utf-8",
    )


def _run_owned_command(command: list[str], environment: dict[str, str]) -> int:
    """Run pytest in owned process group and reap it on interrupt."""
    def interrupt(_signum: int, _frame: object) -> None:
        raise RunnerInterrupted()

    previous_term = signal.signal(signal.SIGTERM, interrupt)
    previous_int = signal.signal(signal.SIGINT, interrupt)
    process = subprocess.Popen(
        command,
        cwd=_SYSTEM_TEST_ROOT.parent,
        env=environment,
        start_new_session=True,
    )
    try:
        return process.wait()
    except RunnerInterrupted:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5.0)
        raise
    finally:
        signal.signal(signal.SIGTERM, previous_term)
        signal.signal(signal.SIGINT, previous_int)


def _run_system_tests(command: list[str], context: NamespaceContext) -> int:
    artifact_root = _artifact_root()
    runner_log = artifact_root / "runner.log"
    try:
        configure_discovery_bridge(SubprocessCommandRunner(), context)
        wait_for_readiness(
            (
                HttpReadinessProbe(context.discovery_host, 3340, "/"),
                HttpReadinessProbe(context.discovery_host, 8080, "/pkarr"),
                DnsUdpReadinessProbe(context.discovery_host, 5300),
            ),
            _DISCOVERY_READY_TIMEOUT_SECONDS,
        )
        environment = os.environ | {
            "SYSTEM_TEST_ARTIFACTS_DIR": str(artifact_root),
            "TELEPATHY_DISCOVERY_LOG_DIR": str(artifact_root),
        }
        returncode = _run_owned_command(command, environment)
        runner_log.write_text(f"pytest exit status: {returncode}\n", encoding="utf-8")
        _capture_artifacts(artifact_root, context, returncode)
        return returncode
    except RunnerInterrupted:
        runner_log.write_text("runner interrupted; owned processes stopped\n", encoding="utf-8")
        _capture_artifacts(artifact_root, context, 130)
        return 130
    except (
        OSError,
        PreflightError,
        ReadinessTimeoutError,
        subprocess.SubprocessError,
    ) as error:
        runner_log.write_text(f"namespace runner failed: {error}\n", encoding="utf-8")
        _capture_artifacts(artifact_root, context, None)
        print(
            f"namespace runner failed; artifacts: {artifact_root}: {error}",
            file=sys.stderr,
        )
        return 2


def main(argv: list[str]) -> int:
    command = argv[1:] if argv[:1] == ["--"] else argv
    try:
        context = _read_context()
        _check_required_commands(context.environment)
        run_preflight(SubprocessCommandRunner(), context)
    except PreflightError as error:
        print(f"namespace preflight failed: {error}", file=sys.stderr)
        return 2

    if not command:
        print(f"namespace preflight passed ({context.environment})")
        return 0
    return _run_system_tests(command, context)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
