from __future__ import annotations

import ipaddress

from harness.topology import TopologyManager


def test_room_worker_id_uses_xdist_worker_not_room_size() -> None:
    assert TopologyManager._parse_worker_index("0-room-3") != 3
    assert TopologyManager._parse_worker_index("7-room-3") != 3
    assert TopologyManager._parse_worker_index("3-room-20") != 20


def test_room_workers_generate_distinct_interface_names() -> None:
    first_gateway, first_client = TopologyManager._interface_names(
        TopologyManager._parse_worker_index("0-room-3"),
        0,
    )
    second_gateway, second_client = TopologyManager._interface_names(
        TopologyManager._parse_worker_index("1-room-3"),
        0,
    )

    assert first_gateway != second_gateway
    assert first_client != second_client
    assert len(first_gateway) <= 15
    assert len(first_client) <= 15


def test_room_twenty_addresses_are_valid_private_30s() -> None:
    worker_index = TopologyManager._parse_worker_index("3-room-20")

    for index in range(20):
        gateway_ip, client_ip = TopologyManager._client_addresses(
            worker_index,
            20,
            index,
        )
        gateway = ipaddress.ip_interface(f"{gateway_ip}/30")
        client = ipaddress.ip_interface(f"{client_ip}/30")

        assert gateway.network == client.network
        assert gateway.ip in ipaddress.ip_network("10.0.0.0/8")
        assert client.ip in ipaddress.ip_network("10.0.0.0/8")
        assert str(gateway.ip) != "10.0.410.1"


def test_parallel_room_topologies_do_not_collide_or_emit_invalid_addresses() -> None:
    interfaces: set[str] = set()
    networks: set[ipaddress.IPv4Network | ipaddress.IPv6Network] = set()

    for room_size in (3, 20):
        for worker in range(8):
            worker_index = TopologyManager._parse_worker_index(
                f"{worker}-room-{room_size}"
            )
            for index in range(room_size):
                gateway_iface, client_iface = TopologyManager._interface_names(
                    worker_index,
                    index,
                )
                gateway_ip, client_ip = TopologyManager._client_addresses(
                    worker_index,
                    room_size,
                    index,
                )
                gateway = ipaddress.ip_interface(f"{gateway_ip}/30")
                client = ipaddress.ip_interface(f"{client_ip}/30")

                assert gateway.network == client.network
                assert gateway.network not in networks
                assert gateway_iface not in interfaces
                assert client_iface not in interfaces

                networks.add(gateway.network)
                interfaces.update((gateway_iface, client_iface))
