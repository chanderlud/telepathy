from __future__ import annotations

import asyncio
import ipaddress

import pytest

from harness.topology import NetworkProfile, TopologyManager


class RecordingTopology(TopologyManager):
    def __init__(self) -> None:
        super().__init__()
        self.commands: list[tuple[str, ...]] = []

    async def _run(self, *args: str) -> None:
        self.commands.append(args)

    async def _delete_namespace_if_exists(self, namespace: str) -> None:
        _ = namespace

    async def _delete_link_if_exists(self, iface: str) -> None:
        _ = iface

    async def _allow_forwarding(self, iface: str) -> None:
        _ = iface

    async def _apply_profile(self, client_namespace: str, profile) -> None:
        _ = profile
        self.commands.append(("apply-profile", client_namespace))


class FailingForwardingTopology(RecordingTopology):
    def __init__(self) -> None:
        super().__init__()
        self.teardown_calls = 0

    async def _run(self, *args: str) -> None:
        self.commands.append(args)
        if "ping" in args:
            raise RuntimeError(
                "command failed: forwarding ping\n"
                "stdout: ping probe output\n"
                "stderr: ping probe failure"
            )

    async def teardown(self) -> None:
        self.teardown_calls += 1
        await super().teardown()


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


def test_given_two_nested_clients_when_topology_starts_then_it_probes_forwarded_reachability() -> None:
    topology = RecordingTopology()

    asyncio.run(
        topology.setup(
            num_clients=2,
            profile=NetworkProfile("satellite", 350, 150, 8, True, 0),
        )
    )

    ping_commands = [command for command in topology.commands if "ping" in command]
    assert len(ping_commands) == 1
    assert ping_commands[0] == (
        "ip",
        "netns",
        "exec",
        "ns-0-cli-0",
        "ping",
        "-c",
        "1",
        "-W",
        "4",
        "10.0.0.6",
    )

    ping_index = topology.commands.index(ping_commands[0])
    profile_index = topology.commands.index(("apply-profile", "ns-0-cli-0"))
    assert ping_index < profile_index


def test_discovery_host_override_uses_slirp_gateway_for_all_services(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("TELEPATHY_DISCOVERY_HOST", "192.0.2.2")
    topology = TopologyManager()
    topology._gateway_ips["ns-0-cli-0"] = "10.0.0.1"

    assert topology.relay_url("ns-0-cli-0") == "http://192.0.2.2:3340"
    assert topology.dns_endpoint("ns-0-cli-0") == "192.0.2.2:5300"
    assert topology.pkarr_relay("ns-0-cli-0") == "http://192.0.2.2:8080/pkarr"


def test_without_discovery_host_override_privileged_topology_uses_host_gateways(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("TELEPATHY_DISCOVERY_HOST", raising=False)
    topology = TopologyManager()
    topology._gateway_ips["ns-0-cli-0"] = "10.0.0.1"

    assert topology.relay_url("ns-0-cli-0") == "http://100.64.0.1:3340"
    assert topology.dns_endpoint("ns-0-cli-0") == "10.0.0.1:5300"
    assert topology.pkarr_relay("ns-0-cli-0") == "http://10.0.0.1:8080/pkarr"


def test_given_forwarding_probe_failure_when_setup_raises_then_live_state_remains_for_capture() -> None:
    topology = FailingForwardingTopology()

    with pytest.raises(RuntimeError) as failure:
        asyncio.run(
            topology.setup(
                num_clients=2,
                profile=NetworkProfile("clean", 0, 0, 0, False, 0),
            )
        )

    assert topology.teardown_calls == 0
    assert topology.client_namespaces == ["ns-0-cli-0", "ns-0-cli-1"]
    assert topology._root_ifaces == ["vr0_0", "vr0_1"]
    assert topology._client_ifaces == {
        "ns-0-cli-0": "vc0_0",
        "ns-0-cli-1": "vc0_1",
    }
    assert "stdout: ping probe output" in str(failure.value)
    assert "stderr: ping probe failure" in str(failure.value)
