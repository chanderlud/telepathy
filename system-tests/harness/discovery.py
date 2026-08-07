from __future__ import annotations

# noqa: SIZE_OK - U2 explicitly requires one cohesive discovery lifecycle module.

import hashlib
import http.client
import os
import platform
import shutil
import socket
import subprocess
import signal
import tarfile
import tempfile
import time
import tomllib
import urllib.request
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Final, Protocol

_SUPPORTED_RUNTIME: Final = "x86_64-unknown-linux-gnu"
_DOWNLOAD_TIMEOUT_SECONDS: Final = 30.0
_DNS_QUERY: Final = (
    b"\x54\x50\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
    b"\x09localhost\x00\x00\x01\x00\x01"
)


@dataclass(frozen=True, slots=True)
class RuntimePlatform:
    system: str
    machine: str

    @classmethod
    def current(cls) -> RuntimePlatform:
        return cls(platform.system(), platform.machine())


@dataclass(frozen=True, slots=True)
class UnsupportedPlatformError(Exception):
    system: str
    machine: str

    def __str__(self) -> str:
        return (
            "discovery binaries support only x86_64 Linux, got "
            f"{self.system} {self.machine}"
        )


@dataclass(frozen=True, slots=True)
class LockFormatError(Exception):
    path: Path
    detail: str

    def __str__(self) -> str:
        return f"invalid discovery lock {self.path}: {self.detail}"


@dataclass(frozen=True, slots=True)
class ArtifactDigestError(Exception):
    name: str
    expected: str
    actual: str

    def __str__(self) -> str:
        return (
            f"{self.name} archive SHA-256 mismatch: expected {self.expected}, "
            f"got {self.actual}"
        )


@dataclass(frozen=True, slots=True)
class UnsafeArchiveError(Exception):
    name: str
    member: str

    def __str__(self) -> str:
        return f"{self.name} archive contains unsafe member {self.member!r}"


@dataclass(frozen=True, slots=True)
class CacheIntegrityError(Exception):
    executable: str
    detail: str

    def __str__(self) -> str:
        return f"cached {self.executable} failed integrity check: {self.detail}"


@dataclass(frozen=True, slots=True)
class ReadinessTimeoutError(Exception):
    timeout: float

    def __str__(self) -> str:
        return f"discovery readiness timed out after {self.timeout:.3f}s"


@dataclass(frozen=True, slots=True)
class ServiceExitedError(Exception):
    name: str
    returncode: int

    def __str__(self) -> str:
        return f"discovery service {self.name} exited with code {self.returncode}"


@dataclass(frozen=True, slots=True)
class LockedArtifact:
    name: str
    version: str
    url: str
    sha256: str
    executable: str


@dataclass(frozen=True, slots=True)
class DiscoveryLock:
    runtime: str
    relay: LockedArtifact
    dns: LockedArtifact


@dataclass(frozen=True, slots=True)
class DiscoveryBinaries:
    relay: Path
    dns: Path


@dataclass(frozen=True, slots=True)
class DiscoveryPaths:
    relay_config: Path
    dns_config: Path
    run_root: Path


class Downloader(Protocol):
    def download(self, url: str, destination: Path) -> None: ...


class UrlDownloader:
    def download(self, url: str, destination: Path) -> None:
        with urllib.request.urlopen(url, timeout=_DOWNLOAD_TIMEOUT_SECONDS) as response:
            with destination.open("wb") as output:
                shutil.copyfileobj(response, output)


class ServiceProcess(Protocol):
    def poll(self) -> int | None: ...
    def terminate(self) -> None: ...
    def kill(self) -> None: ...
    def wait(self, timeout: float) -> int: ...


class ProcessRunner(Protocol):
    def start(
        self, name: str, command: tuple[str, ...], log_path: Path
    ) -> ServiceProcess: ...


class ReadinessProbe(Protocol):
    def ready(self, timeout: float) -> bool: ...


def _parse_artifact(row, lock_path: Path) -> LockedArtifact:
    match row:  # noqa: MATCH_OK - validates untyped TOML boundary data.
        case {
            "name": str(name),
            "version": str(version),
            "url": str(url),
            "sha256": str(sha256),
            "executable": str(executable),
        } if len(sha256) == 64:
            return LockedArtifact(name, version, url, sha256, executable)
        case _:
            raise LockFormatError(lock_path, "artifact fields are missing or invalid")


def load_discovery_lock(
    lock_path: Path, runtime: RuntimePlatform | None = None
) -> DiscoveryLock:
    selected_runtime = RuntimePlatform.current() if runtime is None else runtime
    if selected_runtime.system != "Linux" or selected_runtime.machine != "x86_64":
        raise UnsupportedPlatformError(
            selected_runtime.system, selected_runtime.machine
        )
    raw = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    match raw:  # noqa: MATCH_OK - validates untyped TOML boundary data.
        case {"version": 1, "runtime": str(runtime_name), "artifacts": [*rows]}:
            artifacts = [_parse_artifact(row, lock_path) for row in rows]
        case _:
            raise LockFormatError(
                lock_path, "expected version 1, runtime, and artifacts"
            )
    if runtime_name != _SUPPORTED_RUNTIME:
        raise LockFormatError(lock_path, f"unsupported runtime {runtime_name!r}")
    by_name = {artifact.name: artifact for artifact in artifacts}
    if set(by_name) != {"relay", "dns"}:
        raise LockFormatError(lock_path, "expected exactly relay and dns artifacts")
    return DiscoveryLock(runtime_name, by_name["relay"], by_name["dns"])


def _digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_cached(
    artifact: LockedArtifact, executable: Path, archive: Path
) -> bool:
    if not executable.exists() and not archive.exists():
        return False
    if (
        not executable.is_file()
        or not archive.is_file()
        or not os.access(executable, os.X_OK)
    ):
        raise CacheIntegrityError(
            artifact.executable, "files or executable mode are invalid"
        )
    archive_digest = _digest(archive)
    if archive_digest != artifact.sha256:
        raise CacheIntegrityError(artifact.executable, "archive digest mismatch")
    with tempfile.TemporaryDirectory(dir=executable.parent) as temporary:
        candidate = Path(temporary) / artifact.executable
        _extract_executable(archive, artifact, candidate)
        if _digest(executable) != _digest(candidate):
            raise CacheIntegrityError(artifact.executable, "executable digest mismatch")
    return True


def _extract_executable(
    archive_path: Path, artifact: LockedArtifact, output: Path
) -> None:
    with tarfile.open(archive_path, "r:gz") as archive:
        selected: tarfile.TarInfo | None = None
        for member in archive.getmembers():
            member_path = PurePosixPath(member.name.replace("\\", "/"))
            unsafe_member = (
                member_path.is_absolute()
                or ".." in member_path.parts
                or member.issym()
                or member.islnk()
            )
            if unsafe_member:
                raise UnsafeArchiveError(artifact.name, member.name)
            if member.isfile() and member_path.name == artifact.executable:
                selected = member
        if selected is None:
            raise CacheIntegrityError(
                artifact.executable, "archive is missing executable"
            )
        source = archive.extractfile(selected)
        if source is None:
            raise CacheIntegrityError(
                artifact.executable, "archive executable is unreadable"
            )
        with source, output.open("wb") as destination:
            shutil.copyfileobj(source, destination)
    output.chmod(0o755)


def _install_artifact(
    artifact: LockedArtifact, cache_dir: Path, downloader: Downloader
) -> Path:
    executable = cache_dir / artifact.executable
    cached_archive = cache_dir / f"{artifact.executable}.tar.gz"
    if _verify_cached(artifact, executable, cached_archive):
        return executable
    cache_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=cache_dir) as temporary:
        temporary_root = Path(temporary)
        archive_path = temporary_root / "artifact.tar.gz"
        candidate = temporary_root / artifact.executable
        downloader.download(artifact.url, archive_path)
        actual_digest = _digest(archive_path)
        if actual_digest != artifact.sha256:
            raise ArtifactDigestError(artifact.name, artifact.sha256, actual_digest)
        _extract_executable(archive_path, artifact, candidate)
        os.replace(archive_path, cached_archive)
        os.replace(candidate, executable)
    return executable


def resolve_binaries(
    lock: DiscoveryLock, cache_root: Path, downloader: Downloader | None = None
) -> DiscoveryBinaries:
    source = UrlDownloader() if downloader is None else downloader
    cache_dir = cache_root / "iroh-discovery" / lock.runtime / lock.relay.version
    relay = _install_artifact(lock.relay, cache_dir, source)
    dns = _install_artifact(lock.dns, cache_dir, source)
    return DiscoveryBinaries(relay, dns)


def verify_binaries(lock: DiscoveryLock, cache_root: Path) -> DiscoveryBinaries:
    """Revalidate already-staged binaries without any network access."""
    cache_dir = cache_root / "iroh-discovery" / lock.runtime / lock.relay.version
    relay = cache_dir / lock.relay.executable
    dns = cache_dir / lock.dns.executable
    if not _verify_cached(lock.relay, relay, cache_dir / f"{lock.relay.executable}.tar.gz"):
        raise CacheIntegrityError(lock.relay.executable, "binary is not staged")
    if not _verify_cached(lock.dns, dns, cache_dir / f"{lock.dns.executable}.tar.gz"):
        raise CacheIntegrityError(lock.dns.executable, "binary is not staged")
    return DiscoveryBinaries(relay, dns)


class _SubprocessService:
    def __init__(self, process: subprocess.Popen[bytes], log_file) -> None:
        self._process = process
        self._log_file = log_file

    def poll(self) -> int | None:
        return self._process.poll()

    def terminate(self) -> None:
        os.killpg(self._process.pid, signal.SIGTERM)

    def kill(self) -> None:
        os.killpg(self._process.pid, signal.SIGKILL)

    def wait(self, timeout: float) -> int:
        try:
            return self._process.wait(timeout=timeout)
        finally:
            self._log_file.close()


class SubprocessProcessRunner:
    def start(
        self, name: str, command: tuple[str, ...], log_path: Path
    ) -> ServiceProcess:
        log_file = log_path.open("ab")
        try:
            process = subprocess.Popen(
                command, stdout=log_file, stderr=subprocess.STDOUT, start_new_session=True
            )
        except OSError:
            log_file.close()
            raise
        return _SubprocessService(process, log_file)


@dataclass(frozen=True, slots=True)
class HttpReadinessProbe:
    host: str
    port: int
    path: str

    def ready(self, timeout: float) -> bool:
        connection = http.client.HTTPConnection(self.host, self.port, timeout=timeout)
        try:
            connection.request("GET", self.path)
            response = connection.getresponse()
            response.read()
            return 200 <= response.status < 500
        except (OSError, http.client.HTTPException):
            return False
        finally:
            connection.close()


@dataclass(frozen=True, slots=True)
class DnsUdpReadinessProbe:
    host: str
    port: int

    def ready(self, timeout: float) -> bool:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                client.settimeout(timeout)
                client.sendto(_DNS_QUERY, (self.host, self.port))
                response, _ = client.recvfrom(512)
        except OSError:
            return False
        return (
            len(response) >= 12
            and response[:2] == _DNS_QUERY[:2]
            and bool(response[2] & 0x80)
        )


def _await_readiness(
    probes: tuple[ReadinessProbe, ...],
    services: tuple[tuple[str, ServiceProcess], ...],
    timeout: float,
) -> None:
    deadline = time.monotonic() + timeout
    pending = list(probes)
    while pending:
        for name, process in services:
            returncode = process.poll()
            if returncode is not None:
                raise ServiceExitedError(name, returncode)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ReadinessTimeoutError(timeout)
        probe_timeout = min(0.2, remaining)
        pending = [probe for probe in pending if not probe.ready(probe_timeout)]
        if pending:
            time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))


def wait_for_readiness(probes: tuple[ReadinessProbe, ...], timeout: float) -> None:
    _await_readiness(probes, (), timeout)


class DiscoveryServices:
    """Owns mutable relay and DNS process lifecycle for one test run."""

    def __init__(
        self,
        binaries: DiscoveryBinaries,
        paths: DiscoveryPaths,
        runner: ProcessRunner | None = None,
    ) -> None:
        self._binaries = binaries
        self._paths = paths
        self._runner = SubprocessProcessRunner() if runner is None else runner
        self._services: list[tuple[str, ServiceProcess]] = []

    @property
    def log_paths(self) -> tuple[Path, Path]:
        return self._paths.run_root / "relay.log", self._paths.run_root / "dns.log"

    def start(self) -> None:
        self._paths.run_root.mkdir(parents=True, exist_ok=True)
        relay_command = (
            str(self._binaries.relay),
            f"--config-path={self._paths.relay_config}",
            "--dev",
        )
        dns_command = (
            str(self._binaries.dns),
            "--config",
            str(self._paths.dns_config),
        )
        relay_log, dns_log = self.log_paths
        relay_process = self._runner.start("relay", relay_command, relay_log)
        self._services.append(("relay", relay_process))
        try:
            dns_process = self._runner.start("dns", dns_command, dns_log)
        except OSError:
            self.stop(timeout=5.0)
            raise
        self._services.append(("dns", dns_process))

    def wait_ready(self, probes: tuple[ReadinessProbe, ...], timeout: float) -> None:
        _await_readiness(probes, tuple(self._services), timeout)

    def stop(self, timeout: float) -> None:
        for _, process in reversed(self._services):
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout)
        self._services.clear()
