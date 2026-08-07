from __future__ import annotations

import socket
import threading

import pytest

from harness.discovery import (
    DnsUdpReadinessProbe,
    HttpReadinessProbe,
    ReadinessTimeoutError,
    wait_for_readiness,
)


class NeverReadyProbe:
    def ready(self, timeout: float) -> bool:
        _ = timeout
        return False


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


def test_readiness_reports_timeout() -> None:
    with pytest.raises(ReadinessTimeoutError, match="0.010"):
        wait_for_readiness((NeverReadyProbe(),), timeout=0.01)
