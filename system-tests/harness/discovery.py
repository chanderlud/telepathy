from __future__ import annotations

import http.client
import socket
import time
from dataclasses import dataclass
from typing import Final, Protocol

_DNS_QUERY: Final = (
    b"\x54\x50\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
    b"\x09localhost\x00\x00\x01\x00\x01"
)


@dataclass(frozen=True, slots=True)
class ReadinessTimeoutError(Exception):
    timeout: float

    def __str__(self) -> str:
        return f"discovery readiness timed out after {self.timeout:.3f}s"


class ReadinessProbe(Protocol):
    def ready(self, timeout: float) -> bool: ...


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


def wait_for_readiness(probes: tuple[ReadinessProbe, ...], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    pending = list(probes)
    while pending:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ReadinessTimeoutError(timeout)
        probe_timeout = min(0.2, remaining)
        pending = [probe for probe in pending if not probe.ready(probe_timeout)]
        if pending:
            time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
