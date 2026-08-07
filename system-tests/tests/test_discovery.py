from __future__ import annotations

# noqa: SIZE_OK - U2 requires one focused discovery test module.

import hashlib
import io
import socket
import tarfile
import threading
from dataclasses import dataclass, field
from pathlib import Path

import pytest

from harness.discovery import (
    ArtifactDigestError,
    CacheIntegrityError,
    DiscoveryPaths,
    DiscoveryServices,
    DiscoveryBinaries,
    DnsUdpReadinessProbe,
    HttpReadinessProbe,
    ReadinessTimeoutError,
    RuntimePlatform,
    ServiceExitedError,
    UnsafeArchiveError,
    UnsupportedPlatformError,
    load_discovery_lock,
    resolve_binaries,
    verify_binaries,
    wait_for_readiness,
)

RELAY_URL = (
    "https://github.com/n0-computer/iroh/releases/download/v1.0.2/"
    "iroh-relay-v1.0.2-x86_64-unknown-linux-gnu.tar.gz"
)
DNS_URL = (
    "https://github.com/n0-computer/iroh/releases/download/v1.0.2/"
    "iroh-dns-server-v1.0.2-x86_64-unknown-linux-gnu.tar.gz"
)


def _archive(path: Path, executable: str, *, unsafe: bool = False) -> str:
    with tarfile.open(path, "w:gz") as archive:
        payload = b"#!/bin/sh\nexit 0\n"
        binary = tarfile.TarInfo(f"release/{executable}")
        binary.mode = 0o755
        binary.size = len(payload)
        archive.addfile(binary, io.BytesIO(payload))
        if unsafe:
            traversal = tarfile.TarInfo("../../outside")
            traversal.size = 1
            archive.addfile(traversal, io.BytesIO(b"x"))
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _lock_file(root: Path, relay_digest: str, dns_digest: str) -> Path:
    lock_path = root / "discovery-binaries.lock"
    lock_path.write_text(
        f'''version = 1
runtime = "x86_64-unknown-linux-gnu"

[[artifacts]]
name = "relay"
version = "1.0.2"
url = "{RELAY_URL}"
sha256 = "{relay_digest}"
executable = "iroh-relay"

[[artifacts]]
name = "dns"
version = "1.0.2"
url = "{DNS_URL}"
sha256 = "{dns_digest}"
executable = "iroh-dns-server"
''',
        encoding="utf-8",
    )
    return lock_path


@dataclass(slots=True)  # noqa: MUTABLE_OK
class LocalArchiveDownloader:
    """Records fixture downloads while copying local archives."""

    archives: dict[str, Path]
    calls: list[str] = field(default_factory=list)

    def download(self, url: str, destination: Path) -> None:
        self.calls.append(url)
        destination.write_bytes(self.archives[url].read_bytes())


@dataclass(slots=True)  # noqa: MUTABLE_OK
class FakeProcess:
    """Models mutable subprocess state and ordered lifecycle events."""

    name: str
    events: list[str]
    returncode: int | None = None

    def poll(self) -> int | None:
        return self.returncode

    def terminate(self) -> None:
        self.events.append(f"terminate:{self.name}")
        self.returncode = 0

    def kill(self) -> None:
        self.events.append(f"kill:{self.name}")
        self.returncode = -9

    def wait(self, timeout: float) -> int:
        self.events.append(f"wait:{self.name}")
        return 0 if self.returncode is None else self.returncode


@dataclass(slots=True)  # noqa: MUTABLE_OK
class FakeProcessRunner:
    """Records process starts and owns fake process fixtures."""

    commands: list[tuple[str, ...]] = field(default_factory=list)
    logs: list[Path] = field(default_factory=list)
    events: list[str] = field(default_factory=list)
    processes: list[FakeProcess] = field(default_factory=list)

    def start(
        self, name: str, command: tuple[str, ...], log_path: Path
    ) -> FakeProcess:
        self.commands.append(command)
        self.logs.append(log_path)
        process = FakeProcess(name, self.events)
        self.processes.append(process)
        return process


@dataclass(frozen=True, slots=True)
class NeverReadyProbe:
    def ready(self, timeout: float) -> bool:
        return False


def test_official_lock_pins_v1_0_2_release_facts() -> None:
    lock_path = Path(__file__).parents[1] / "discovery-binaries.lock"

    lock = load_discovery_lock(
        lock_path, RuntimePlatform(system="Linux", machine="x86_64")
    )

    assert lock.relay.url == RELAY_URL
    assert lock.relay.sha256 == (
        "7faf12b2b0137b5993e8dd1fb7557b2e61fee1a53486db74bb80d5c96907af93"
    )
    assert lock.dns.url == DNS_URL
    assert lock.dns.sha256 == (
        "5d5221d4494deb6c69b42e506feb1ad9db63458a20434100a2fc1dbd4fcd0faa"
    )


def test_lock_rejects_unsupported_runtime_architecture(tmp_path: Path) -> None:
    lock_path = _lock_file(tmp_path, "0" * 64, "1" * 64)

    with pytest.raises(UnsupportedPlatformError, match="x86_64 Linux"):
        load_discovery_lock(
            lock_path, RuntimePlatform(system="Linux", machine="aarch64")
        )


def test_empty_cache_installs_verified_executables_without_network(
    tmp_path: Path,
) -> None:
    relay_archive = tmp_path / "relay.tar.gz"
    dns_archive = tmp_path / "dns.tar.gz"
    lock_path = _lock_file(
        tmp_path,
        _archive(relay_archive, "iroh-relay"),
        _archive(dns_archive, "iroh-dns-server"),
    )
    downloader = LocalArchiveDownloader(
        {RELAY_URL: relay_archive, DNS_URL: dns_archive}
    )

    binaries = resolve_binaries(
        load_discovery_lock(lock_path, RuntimePlatform("Linux", "x86_64")),
        tmp_path / "cache",
        downloader,
    )

    assert binaries.relay.read_bytes().startswith(b"#!/bin/sh")
    assert binaries.dns.read_bytes().startswith(b"#!/bin/sh")
    assert binaries.relay.stat().st_mode & 0o111
    assert resolve_binaries(
        load_discovery_lock(lock_path, RuntimePlatform("Linux", "x86_64")),
        tmp_path / "cache",
        downloader,
    ) == binaries
    assert downloader.calls == [RELAY_URL, DNS_URL]


def test_cache_hit_revalidates_executable_digest(tmp_path: Path) -> None:
    relay_archive = tmp_path / "relay.tar.gz"
    dns_archive = tmp_path / "dns.tar.gz"
    lock = load_discovery_lock(
        _lock_file(
            tmp_path,
            _archive(relay_archive, "iroh-relay"),
            _archive(dns_archive, "iroh-dns-server"),
        ),
        RuntimePlatform("Linux", "x86_64"),
    )
    downloader = LocalArchiveDownloader(
        {RELAY_URL: relay_archive, DNS_URL: dns_archive}
    )
    binaries = resolve_binaries(lock, tmp_path / "cache", downloader)
    binaries.relay.write_bytes(b"tampered")

    with pytest.raises(CacheIntegrityError, match="iroh-relay"):
        resolve_binaries(lock, tmp_path / "cache", downloader)

    assert downloader.calls == [RELAY_URL, DNS_URL]


def test_given_staged_cache_when_inner_runner_verifies_then_no_download_occurs(
    tmp_path: Path,
) -> None:
    relay_archive = tmp_path / "relay.tar.gz"
    dns_archive = tmp_path / "dns.tar.gz"
    lock = load_discovery_lock(
        _lock_file(
            tmp_path,
            _archive(relay_archive, "iroh-relay"),
            _archive(dns_archive, "iroh-dns-server"),
        ),
        RuntimePlatform("Linux", "x86_64"),
    )
    downloader = LocalArchiveDownloader({RELAY_URL: relay_archive, DNS_URL: dns_archive})
    cache = tmp_path / "cache"

    staged = resolve_binaries(lock, cache, downloader)
    verified = verify_binaries(lock, cache)

    assert verified == staged
    assert downloader.calls == [RELAY_URL, DNS_URL]


def test_archive_digest_and_traversal_are_rejected(tmp_path: Path) -> None:
    relay_archive = tmp_path / "relay.tar.gz"
    dns_archive = tmp_path / "dns.tar.gz"
    relay_digest = _archive(relay_archive, "iroh-relay", unsafe=True)
    dns_digest = _archive(dns_archive, "iroh-dns-server")
    downloader = LocalArchiveDownloader(
        {RELAY_URL: relay_archive, DNS_URL: dns_archive}
    )

    bad_lock = load_discovery_lock(
        _lock_file(tmp_path, "f" * 64, dns_digest),
        RuntimePlatform("Linux", "x86_64"),
    )
    with pytest.raises(ArtifactDigestError):
        resolve_binaries(bad_lock, tmp_path / "bad-cache", downloader)

    safe_digest_lock = load_discovery_lock(
        _lock_file(tmp_path, relay_digest, dns_digest),
        RuntimePlatform("Linux", "x86_64"),
    )
    with pytest.raises(UnsafeArchiveError):
        resolve_binaries(safe_digest_lock, tmp_path / "unsafe-cache", downloader)


def test_services_use_current_commands_logs_and_reverse_shutdown(
    tmp_path: Path,
) -> None:
    relay = tmp_path / "iroh-relay"
    dns = tmp_path / "iroh-dns-server"
    relay.touch(mode=0o755)
    dns.touch(mode=0o755)
    (tmp_path / "relay.toml").touch()
    (tmp_path / "dns.toml").touch()
    runner = FakeProcessRunner()
    paths = DiscoveryPaths(
        relay_config=tmp_path / "relay.toml",
        dns_config=tmp_path / "dns.toml",
        run_root=tmp_path / "run",
    )

    services = DiscoveryServices(DiscoveryBinaries(relay, dns), paths, runner)
    services.start()
    services.stop(timeout=1.0)

    assert runner.commands == [
        (str(relay), f"--config-path={paths.relay_config}", "--dev"),
        (str(dns), "--config", str(paths.dns_config)),
    ]
    assert runner.logs == [
        paths.run_root / "relay.log",
        paths.run_root / "dns.log",
    ]
    assert runner.events == [
        "terminate:dns",
        "wait:dns",
        "terminate:relay",
        "wait:relay",
    ]


def test_bounded_http_and_dns_udp_readiness_use_live_sockets() -> None:
    tcp = socket.socket()
    tcp.bind(("127.0.0.1", 0))
    tcp.listen()
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(("127.0.0.1", 0))
    tcp_port = tcp.getsockname()[1]
    udp_port = udp.getsockname()[1]

    def serve_http() -> None:
        connection, _ = tcp.accept()
        connection.recv(4096)
        connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        connection.close()

    def serve_dns() -> None:
        request, address = udp.recvfrom(512)
        udp.sendto(request[:2] + b"\x81\x80" + request[4:], address)

    http_thread = threading.Thread(target=serve_http)
    dns_thread = threading.Thread(target=serve_dns)
    http_thread.start()
    dns_thread.start()
    try:
        wait_for_readiness(
            (
                HttpReadinessProbe("127.0.0.1", tcp_port, "/"),
                DnsUdpReadinessProbe("127.0.0.1", udp_port),
            ),
            timeout=2.0,
        )
    finally:
        http_thread.join(timeout=2.0)
        dns_thread.join(timeout=2.0)
        tcp.close()
        udp.close()


def test_readiness_reports_timeout_and_early_service_exit(tmp_path: Path) -> None:
    with pytest.raises(ReadinessTimeoutError):
        wait_for_readiness((NeverReadyProbe(),), timeout=0.01)

    relay = tmp_path / "iroh-relay"
    dns = tmp_path / "iroh-dns-server"
    runner = FakeProcessRunner()
    services = DiscoveryServices(
        DiscoveryBinaries(relay, dns),
        DiscoveryPaths(tmp_path / "relay.toml", tmp_path / "dns.toml", tmp_path),
        runner,
    )
    services.start()
    runner.processes[0].returncode = 17

    with pytest.raises(ServiceExitedError, match="relay exited with code 17"):
        services.wait_ready((NeverReadyProbe(),), timeout=1.0)
