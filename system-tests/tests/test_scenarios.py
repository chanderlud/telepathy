from __future__ import annotations

import asyncio
import base64
import json
import os
import re
import sys
import urllib.error
import urllib.request
from collections.abc import AsyncIterator
from dataclasses import dataclass
from datetime import datetime
from inspect import unwrap
from pathlib import Path

import pytest
import pytest_asyncio
import zbase32
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from harness.process import CliProcess
from harness.scenario import ScenarioRunner
from harness.topology import NetworkProfile, TopologyManager


SCENARIOS_ROOT = Path(__file__).resolve().parents[1] / "scenarios"


@dataclass(frozen=True)
class Identity:
    secret_key_b64: str
    peer_id_hex: str


@dataclass(frozen=True)
class RoomCliGroup:
    actors: dict[str, CliProcess]
    topology: TopologyManager

NETWORK_PROFILES = [
    NetworkProfile(
        name="clean",
        delay_ms=0,
        jitter_ms=0,
        loss_pct=0.0,
        burst_loss=False,
        seed=100,
    ),
    NetworkProfile(
        name="wan",
        delay_ms=80,
        jitter_ms=20,
        loss_pct=0.2,
        burst_loss=False,
        seed=101,
    ),
    NetworkProfile(
        name="bad_mobile",
        delay_ms=250,
        jitter_ms=100,
        loss_pct=3.0,
        burst_loss=True,
        seed=102,
    ),
    NetworkProfile(
        name="satellite",
        delay_ms=350,
        jitter_ms=150,
        loss_pct=8.0,
        burst_loss=True,
        seed=103,
    ),
]

REPEATED_CALL_PROFILE = NetworkProfile(
    name="repeated_call_moderate_delay_zero_loss",
    delay_ms=300,
    jitter_ms=0,
    loss_pct=0.0,
    burst_loss=False,
    seed=104,
)

ROOM_THREE_ACTORS = ["alice", "bob", "carol"]
ROOM_FOUR_ACTORS = ["alice", "bob", "carol", "dave"]
ROOM_TWENTY_ACTORS = [f"peer{index:02d}" for index in range(1, 21)]
FULL_LOSS_SAMPLES_PER_STAT = 4_800
MAX_CONSECUTIVE_FULL_LOSS_STATS = 20


def _generate_identity() -> Identity:
    private_key = Ed25519PrivateKey.generate()
    secret_key = private_key.private_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PrivateFormat.Raw,
        encryption_algorithm=serialization.NoEncryption(),
    )
    public_key = private_key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    )
    return Identity(
        secret_key_b64=base64.b64encode(secret_key).decode("ascii"),
        peer_id_hex=public_key.hex(),
    )


async def _set_identity(actor: CliProcess, identity: Identity) -> str:
    response = await actor.send(
        {
            "cmd": "set_identity",
            "args": {"key_b64": identity.secret_key_b64},
        }
    )
    if not response.get("ok"):
        raise AssertionError(f"set_identity failed: {response}")
    return identity.peer_id_hex


async def _set_identity_on_actor(actor: CliProcess, identity: Identity) -> str:
    peer_id = await _set_identity(actor, identity)
    actor.identity = identity
    actor.identity_peer_id = peer_id
    return peer_id


_MANAGER_STATES = ("Stopped", "Starting", "Active", "Failed")


def _manager_state_from_event(message: dict) -> str | None:
    if message.get("kind") != "event" or message.get("type") != "manager_active":
        return None

    matched = [state for state in _MANAGER_STATES if state in message]
    if len(matched) != 1:
        return None
    return matched[0]


def _peer_id_to_z32(peer_id_hex: str) -> str:
    encode = getattr(zbase32, "encode")
    return encode(bytes.fromhex(peer_id_hex))


def _pkarr_record_url(relay_base: str, peer_id_hex: str) -> str:
    z32_key = _peer_id_to_z32(peer_id_hex)
    return f"{relay_base.rstrip('/')}/{z32_key}"


async def _fetch_http_status(url: str) -> int | None:
    def _fetch() -> int | None:
        try:
            with urllib.request.urlopen(url, timeout=2) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code
        except urllib.error.URLError:
            return None

    return await asyncio.to_thread(_fetch)


async def wait_for_pkarr_published(
    peers: list[tuple[CliProcess, str]],
    topology: TopologyManager,
    timeout: float = 15.0,
) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout

    pending: dict[str, tuple[str, str]] = {}
    for actor, namespace in peers:
        peer_id = actor.identity_peer_id
        if not isinstance(peer_id, str):
            raise AssertionError("cli process missing identity_peer_id before pkarr wait")
        relay_base = topology.pkarr_relay(namespace)
        url = _pkarr_record_url(relay_base, peer_id)
        pending[url] = (namespace, peer_id)

    while loop.time() < deadline and pending:
        for url in list(pending.keys()):
            status = await _fetch_http_status(url)
            if status == 200:
                del pending[url]
        if pending:
            await asyncio.sleep(0.5)

    if not pending:
        return

    details = "\n".join(
        f"  namespace={namespace} peer_id={peer_id} url={url}"
        for url, (namespace, peer_id) in pending.items()
    )
    raise AssertionError(
        f"pkarr records not published within {timeout}s timeout:\n{details}"
    )


def _sanitize_nodeid(nodeid: str) -> str:
    safe = []
    for char in nodeid:
        if char.isalnum() or char in ("-", "_", "."):
            safe.append(char)
        else:
            safe.append("_")
    return "".join(safe)


async def capture_dns_server_logs(timeout: float = 5.0) -> str:
    _ = timeout
    log_directory = os.environ.get("TELEPATHY_DISCOVERY_LOG_DIR")
    if log_directory is None:
        return "discovery log directory was not provided by namespace runner"
    log_path = Path(log_directory) / "dns.log"
    if not log_path.is_file():
        return f"discovery DNS log is unavailable: {log_path}"
    return log_path.read_text(encoding="utf-8", errors="replace")


async def capture_live_topology_artifacts(
    topology: TopologyManager, artifact_dir: Path, iteration_id: str
) -> None:
    commands = {
        "namespaces.txt": ("ip", "netns", "list"),
        "links.txt": ("ip", "-details", "link", "show"),
        "addresses.txt": ("ip", "address", "show"),
        "routes.txt": ("ip", "route", "show", "table", "all"),
        "neighbors.txt": ("ip", "neighbor", "show"),
        "forwarding.txt": ("iptables", "-S", "FORWARD"),
        "qdiscs.txt": ("tc", "qdisc", "show"),
        "ip-forward.txt": ("sysctl", "net.ipv4.ip_forward"),
    }
    for namespace in topology.client_namespaces:
        namespace_prefix = ("ip", "netns", "exec", namespace)
        commands.update(
            {
                f"{namespace}-links.txt": namespace_prefix
                + ("ip", "-details", "link", "show"),
                f"{namespace}-addresses.txt": namespace_prefix
                + ("ip", "address", "show"),
                f"{namespace}-routes.txt": namespace_prefix
                + ("ip", "route", "show", "table", "all"),
                f"{namespace}-neighbors.txt": namespace_prefix
                + ("ip", "neighbor", "show"),
                f"{namespace}-qdiscs.txt": namespace_prefix
                + ("tc", "qdisc", "show"),
            }
        )
    for name, command in commands.items():
        command_text = " ".join(command)
        try:
            process = await asyncio.create_subprocess_exec(
                *command,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await process.communicate()
            payload = (
                f"$ {command_text}\n"
                f"return code: {process.returncode}\n"
                f"--- stdout ---\n{stdout.decode('utf-8', errors='replace')}\n"
                f"--- stderr ---\n{stderr.decode('utf-8', errors='replace')}\n"
            )
        except OSError as error:
            payload = f"$ {command_text}\nsnapshot failed: {error}\n"
        (artifact_dir / name).write_text(payload, encoding="utf-8")
    (artifact_dir / "topology-context.json").write_text(
        json.dumps(
            {
                "worker": os.environ.get("PYTEST_XDIST_WORKER", "0"),
                "iteration": iteration_id,
                "namespaces": topology.client_namespaces,
            },
            indent=2,
        ),
        encoding="utf-8",
    )


@pytest.mark.asyncio
async def test_given_live_topology_when_snapshot_commands_fail_then_bundle_is_complete(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class SnapshotProcess:
        def __init__(
            self, command: tuple[str, ...], returncode: int = 0
        ) -> None:
            self.command = command
            self.returncode = returncode

        async def communicate(self) -> tuple[bytes, bytes]:
            stdout = f"stdout for {' '.join(self.command)}".encode()
            stderr = b"iptables snapshot failed" if self.returncode else b""
            return stdout, stderr

    async def create_snapshot_process(*command: str, **kwargs) -> SnapshotProcess:
        _ = kwargs
        if command == ("tc", "qdisc", "show"):
            raise FileNotFoundError("tc snapshot unavailable")
        return SnapshotProcess(command, returncode=4 if command[0] == "iptables" else 0)

    topology = TopologyManager(worker_id="gw3")
    topology.client_namespaces = ["ns-gw3-cli-0", "ns-gw3-cli-1"]
    monkeypatch.setenv("PYTEST_XDIST_WORKER", "gw3")
    monkeypatch.setattr(asyncio, "create_subprocess_exec", create_snapshot_process)

    await capture_live_topology_artifacts(topology, tmp_path, "iteration-7")

    expected_files = {
        "namespaces.txt",
        "links.txt",
        "addresses.txt",
        "routes.txt",
        "neighbors.txt",
        "forwarding.txt",
        "qdiscs.txt",
        "ip-forward.txt",
        "ns-gw3-cli-0-links.txt",
        "ns-gw3-cli-0-addresses.txt",
        "ns-gw3-cli-0-routes.txt",
        "ns-gw3-cli-0-neighbors.txt",
        "ns-gw3-cli-0-qdiscs.txt",
        "ns-gw3-cli-1-links.txt",
        "ns-gw3-cli-1-addresses.txt",
        "ns-gw3-cli-1-routes.txt",
        "ns-gw3-cli-1-neighbors.txt",
        "ns-gw3-cli-1-qdiscs.txt",
        "topology-context.json",
    }
    assert expected_files <= {path.name for path in tmp_path.iterdir()}
    assert all((tmp_path / name).read_text(encoding="utf-8") for name in expected_files)
    assert "return code: 4" in (tmp_path / "forwarding.txt").read_text()
    assert "iptables snapshot failed" in (tmp_path / "forwarding.txt").read_text()
    assert "tc snapshot unavailable" in (tmp_path / "qdiscs.txt").read_text()
    assert json.loads((tmp_path / "topology-context.json").read_text()) == {
        "worker": "gw3",
        "iteration": "iteration-7",
        "namespaces": ["ns-gw3-cli-0", "ns-gw3-cli-1"],
    }


@pytest.mark.asyncio
async def test_given_setup_failure_when_fixture_exits_then_capture_precedes_single_cleanup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []

    class FailingTopology:
        def __init__(self, worker_id: str) -> None:
            self.worker_id = worker_id
            self.client_namespaces = ["ns-gw4-cli-0", "ns-gw4-cli-1"]

        async def setup(self, num_clients: int, profile: NetworkProfile) -> None:
            _ = num_clients, profile
            raise RuntimeError(
                "command failed: forwarding ping\n"
                "stdout: ping probe output\n"
                "stderr: ping probe failure"
            )

        async def teardown(self) -> None:
            events.append("teardown")
            self.client_namespaces = []

    class Config:
        def getoption(self, name: str) -> str:
            assert name == "artifacts_dir"
            return str(tmp_path)

    class Node:
        nodeid = "tests/test_scenarios.py::forwarding-probe"
        rep_call = None

    class Request:
        config = Config()
        node = Node()

    async def capture(
        manager: FailingTopology, artifact_dir: Path, iteration_id: str
    ) -> None:
        assert manager.client_namespaces == ["ns-gw4-cli-0", "ns-gw4-cli-1"]
        assert iteration_id == "iteration-9"
        assert "stdout: ping probe output" in (
            artifact_dir / "setup-error.txt"
        ).read_text()
        assert "stderr: ping probe failure" in (
            artifact_dir / "setup-error.txt"
        ).read_text()
        events.append("capture")

    module = sys.modules[__name__]
    monkeypatch.setattr(module, "TopologyManager", FailingTopology)
    monkeypatch.setattr(module, "capture_live_topology_artifacts", capture)
    fixture = unwrap(topology)(
        NetworkProfile("clean", 0, 0, 0, False, 0),
        "gw4",
        Request(),
        "iteration-9",
    )

    with pytest.raises(RuntimeError, match="forwarding ping"):
        await fixture.__anext__()

    assert events == ["capture", "teardown"]
    cleanup_files = list(tmp_path.glob("*/cleanup.json"))
    assert len(cleanup_files) == 1
    assert json.loads(cleanup_files[0].read_text()) == {
        "topology_teardown": "completed"
    }


async def wait_for_relay_ready(actor: CliProcess, timeout: float = 30.0) -> dict:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while loop.time() < deadline:
        for message in actor.stdout_lines():
            if _manager_state_from_event(message) == "Active":
                return message
        await asyncio.sleep(0.05)

    transcript = "\n".join(str(message) for message in actor.stdout_lines())
    raise AssertionError(
        "manager_active Active event not observed within timeout.\n"
        f"Captured stdout transcript:\n{transcript}"
    )


def _status_name(status: object) -> str | None:
    if isinstance(status, str):
        return status
    if isinstance(status, dict) and len(status) == 1:
        only_key = next(iter(status.keys()))
        if isinstance(only_key, str):
            return only_key
    return None


def _room_peer_from_state(message: dict, state_name: str) -> str | None:
    if message.get("kind") != "event" or message.get("type") != "call_state":
        return None
    state = message.get("state")
    if isinstance(state, dict) and state.get(state_name) is not None:
        peer = state.get(state_name)
        if isinstance(peer, str):
            return peer
    return None


def _latest_session_statuses(messages: list[dict]) -> dict[str, str]:
    statuses: dict[str, str] = {}
    for message in messages:
        if message.get("kind") != "event" or message.get("type") != "session_status":
            continue
        peer = message.get("peer")
        if not isinstance(peer, str):
            continue
        status_name = _status_name(message.get("status"))
        if status_name:
            statuses[peer] = status_name
    return statuses


def _settled_room_members(messages: list[dict]) -> set[str]:
    members: set[str] = set()
    for message in messages:
        joined = _room_peer_from_state(message, "RoomJoin")
        if joined is not None:
            members.add(joined)
            continue
        left = _room_peer_from_state(message, "RoomLeave")
        if left is not None:
            members.discard(left)
    return members


def _error_events(messages: list[dict]) -> list[dict]:
    return [
        message
        for message in messages
        if message.get("kind") == "event" and message.get("type") in {"error", "Error"}
    ]


def _room_mesh_missing(actors: dict[str, CliProcess]) -> list[str]:
    peer_ids = {
        name: actor.identity_peer_id
        for name, actor in actors.items()
        if isinstance(actor.identity_peer_id, str)
    }
    missing: list[str] = []

    for name, actor in actors.items():
        own_peer_id = peer_ids.get(name)
        if own_peer_id is None:
            missing.append(f"{name}: missing identity_peer_id")
            continue

        expected_peers = {peer_id for peer_id in peer_ids.values() if peer_id != own_peer_id}
        messages = actor.stdout_lines()
        if errors := _error_events(messages):
            missing.append(f"{name}: emitted error events {errors}")

        statuses = _latest_session_statuses(messages)
        connected = {
            peer_id
            for peer_id, status in statuses.items()
            if peer_id in expected_peers and status == "Connected"
        }
        if connected != expected_peers:
            missing.append(
                f"{name}: connected peers missing {sorted(expected_peers - connected)}; "
                f"unexpected statuses {statuses}"
            )

        room_members = _settled_room_members(messages)
        if room_members != expected_peers:
            missing.append(
                f"{name}: settled room members expected {sorted(expected_peers)} "
                f"got {sorted(room_members)}"
            )

    return missing


async def wait_for_room_mesh_connected(
    actors: dict[str, CliProcess],
    *,
    timeout: float = 120.0,
    stability_window: float = 2.0,
) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    last_missing: list[str] = []

    while loop.time() < deadline:
        last_missing = _room_mesh_missing(actors)
        if not last_missing:
            await asyncio.sleep(stability_window)
            settled_missing = _room_mesh_missing(actors)
            if not settled_missing:
                return
            last_missing = settled_missing
        await asyncio.sleep(0.2)

    diagnostics = "\n".join(last_missing) if last_missing else "no diagnostics captured"
    raise AssertionError(f"room mesh did not settle within {timeout:.1f}s:\n{diagnostics}")


async def _start_cli_group(
    *,
    topology: TopologyManager,
    binaries: dict[str, str],
    actor_names: list[str],
) -> dict[str, CliProcess]:
    actors: dict[str, CliProcess] = {}
    for actor_name, namespace in zip(actor_names, topology.client_namespaces):
        actors[actor_name] = CliProcess(
            binary_path=binaries["cli"],
            namespace=namespace,
            listen_port=0,
            bind_addresses=["0.0.0.0"],
            relay_url=topology.relay_url(namespace),
            dns_endpoint=topology.dns_endpoint(namespace),
            dns_origin_domain=topology.dns_origin_domain(namespace),
            pkarr_relay=topology.pkarr_relay(namespace),
        )

    await asyncio.gather(*(actor.start() for actor in actors.values()))
    await asyncio.gather(
        *(_set_identity_on_actor(actor, _generate_identity()) for actor in actors.values())
    )
    start_manager_responses = await asyncio.gather(
        *(actor.send({"cmd": "start_manager", "args": {}}) for actor in actors.values())
    )
    for actor_name, response in zip(actors, start_manager_responses):
        assert response.get("ok") is True, f"{actor_name} start_manager failed: {response}"

    await asyncio.gather(*(wait_for_relay_ready(actor) for actor in actors.values()))
    await wait_for_pkarr_published(
        [(actor, actor.namespace) for actor in actors.values()],
        topology,
        timeout=30.0,
    )
    return actors


async def _terminate_actors(actors: dict[str, CliProcess]) -> None:
    await asyncio.gather(*(actor.terminate() for actor in reversed(list(actors.values()))))


async def _add_contact(owner: CliProcess, contact_id: str, peer_id: str) -> None:
    response = await owner.send(
        {
            "cmd": "add_contact",
            "args": {"contact_id": contact_id, "peer_id": peer_id},
        }
    )
    assert response.get("ok") is True, f"add_contact {contact_id} failed: {response}"


async def _add_all_contacts(actors: dict[str, CliProcess]) -> None:
    tasks = []
    for owner_name, owner in actors.items():
        for contact_name, contact in actors.items():
            if contact_name == owner_name:
                continue
            peer_id = contact.identity_peer_id
            if not isinstance(peer_id, str):
                raise AssertionError(f"{contact_name} missing identity_peer_id")
            tasks.append(_add_contact(owner, contact_name, peer_id))
    await asyncio.gather(*tasks)


async def _add_sparse_room_contacts(actors: dict[str, CliProcess]) -> None:
    actor_items = list(actors.items())
    tasks = []
    for index, (owner_name, owner) in enumerate(actor_items[:10]):
        contact_names = {
            actor_items[(index + 1) % len(actor_items)][0],
            actor_items[(index + 5) % len(actor_items)][0],
        }
        for contact_name in contact_names:
            contact = actors[contact_name]
            peer_id = contact.identity_peer_id
            if not isinstance(peer_id, str):
                raise AssertionError(f"{contact_name} missing identity_peer_id")
            tasks.append(_add_contact(owner, contact_name, peer_id))
    await asyncio.gather(*tasks)


async def _join_room_from_all(actors: dict[str, CliProcess], *, timeout: float = 60.0) -> None:
    members = _room_member_peer_ids(actors)

    responses = await asyncio.gather(
        *(
            asyncio.wait_for(
                actor.send({"cmd": "join_room", "args": {"members": members}}),
                timeout=timeout,
            )
            for actor in actors.values()
        )
    )
    for actor_name, response in zip(actors, responses):
        assert response.get("ok") is True, f"{actor_name} join_room failed: {response}"


def _room_member_peer_ids(actors: dict[str, CliProcess]) -> list[str]:
    members = sorted(
        peer_id
        for actor in actors.values()
        if isinstance((peer_id := actor.identity_peer_id), str)
    )
    if len(members) != len(actors):
        raise AssertionError("cannot build room members before all actors have identities")
    return members


async def _join_room_from_actor(
    actor_name: str,
    actor: CliProcess,
    members: list[str],
    *,
    timeout: float = 60.0,
) -> dict:
    response = await asyncio.wait_for(
        actor.send({"cmd": "join_room", "args": {"members": members}}),
        timeout=timeout,
    )
    assert response.get("ok") is True, f"{actor_name} join_room failed: {response}"
    return response


def _assert_ack_error_contains(response: dict, expected: str) -> None:
    assert response.get("ok") is False, f"expected failed ack, got {response}"
    error = response.get("error")
    assert isinstance(error, str), f"expected string error in {response}"
    assert expected in error, f"expected {expected!r} in error {error!r}"


def _has_error_event(messages: list[dict]) -> bool:
    for message in messages:
        if message.get("kind") != "event":
            continue
        if message.get("type") in {"Error", "error"}:
            return True
    return False


def _call_state_name(state: object) -> str | None:
    if isinstance(state, str):
        return state
    if isinstance(state, dict) and len(state) == 1:
        only_key = next(iter(state.keys()))
        if isinstance(only_key, str):
            return only_key
    return None


def _is_call_state(event: dict, state_name: str) -> bool:
    if event.get("type") != "call_state":
        return False
    return _call_state_name(event.get("state")) == state_name


def _statistics_since(actor: CliProcess, start_index: int) -> list[dict]:
    return [
        message
        for message in actor.stdout_lines()[start_index:]
        if message.get("kind") == "event" and message.get("type") == "statistics"
    ]


def _max_stat_value(stats: list[dict], field: str) -> float:
    values = [
        value
        for stat in stats
        if isinstance((value := stat.get(field)), (int, float))
    ]
    return float(max(values, default=0.0))


def _longest_full_loss_run(stats: list[dict]) -> int:
    longest = 0
    current = 0
    for stat in stats:
        loss = stat.get("loss")
        if isinstance(loss, int) and loss >= FULL_LOSS_SAMPLES_PER_STAT:
            current += 1
            longest = max(longest, current)
        else:
            current = 0
    return longest


def _has_sustained_full_loss_then_recovery(stats: list[dict]) -> bool:
    current = 0
    for stat in stats:
        loss = stat.get("loss")
        if isinstance(loss, int) and loss >= FULL_LOSS_SAMPLES_PER_STAT:
            current += 1
            continue
        if current >= MAX_CONSECUTIVE_FULL_LOSS_STATS:
            return True
        current = 0
    return False


async def _accept_next_call(callee: CliProcess, contact_id: str) -> None:
    prompt = await callee.expect_event(
        lambda event: event.get("type") == "accept_call_prompt"
        and event.get("contact_id") == contact_id,
        timeout=20.0,
    )
    request_id = prompt.get("request_id")
    assert isinstance(request_id, str), f"accept_call_prompt missing request_id: {prompt}"
    response = await callee.send(
        {"cmd": "accept_call", "args": {"request_id": request_id, "accept": True}}
    )
    assert response.get("ok") is True, f"accept_call failed: {response}"


async def _start_call_and_wait_connected(
    caller: CliProcess,
    callee: CliProcess,
    *,
    caller_contact_id: str,
    callee_contact_id: str,
) -> None:
    response = await caller.send(
        {"cmd": "start_call", "args": {"contact_id": caller_contact_id}}
    )
    assert response.get("ok") is True, f"start_call failed: {response}"
    await _accept_next_call(callee, callee_contact_id)
    await asyncio.gather(
        caller.expect_event(lambda event: _is_call_state(event, "Connected"), timeout=20.0),
        callee.expect_event(lambda event: _is_call_state(event, "Connected"), timeout=20.0),
    )


async def _collect_statistics_window(
    actor: CliProcess,
    *,
    duration: float,
) -> list[dict]:
    start_index = len(actor.stdout_lines())
    await asyncio.sleep(duration)
    return _statistics_since(actor, start_index)


def _assert_no_sustained_full_loss(actor_name: str, stats: list[dict]) -> None:
    longest_run = _longest_full_loss_run(stats)
    max_loss = _max_stat_value(stats, "loss")
    max_download = _max_stat_value(stats, "download_bandwidth")

    assert not _has_sustained_full_loss_then_recovery(stats), (
        f"{actor_name} recorded issue #44 sustained full-loss then recovery; "
        f"longest_run={longest_run}, "
        f"max_loss={max_loss}, max_download_bandwidth={max_download}, stats={stats}"
    )


async def _room_cli_group_fixture(
    *,
    profile: NetworkProfile,
    worker_tag: str,
    binaries: dict[str, str],
    actor_names: list[str],
) -> AsyncIterator[RoomCliGroup]:
    topology = TopologyManager(worker_id=f"{worker_tag}-room-{len(actor_names)}")
    actors: dict[str, CliProcess] = {}
    try:
        await topology.setup(num_clients=len(actor_names), profile=profile)
        if not topology.client_namespaces:
            pytest.skip(
                "topology setup did not create client namespaces; "
                "network namespace privileges (CAP_NET_ADMIN / ip netns) are required"
            )
        actors = await _start_cli_group(
            topology=topology,
            binaries=binaries,
            actor_names=actor_names,
        )
        yield RoomCliGroup(actors=actors, topology=topology)
    finally:
        if actors:
            await _terminate_actors(actors)
        await topology.teardown()


@pytest_asyncio.fixture
async def topology(
    profile: NetworkProfile,
    worker_tag: str,
    request: pytest.FixtureRequest,
    iteration_id: str,
) -> AsyncIterator[TopologyManager]:
    manager = TopologyManager(worker_id=worker_tag)
    test_failed = False
    setup_failure: str | None = None
    artifact_dir: Path | None = None
    try:
        await manager.setup(num_clients=2, profile=profile)
        if not manager.client_namespaces:
            pytest.skip(
                "topology setup did not create client namespaces; "
                "network namespace privileges (CAP_NET_ADMIN / ip netns) are required"
            )
        yield manager
    except Exception as error:
        test_failed = True
        setup_failure = str(error)
        raise
    finally:
        call_report = getattr(request.node, "rep_call", None)
        if test_failed or getattr(call_report, "failed", False):
            artifacts_root = Path(str(request.config.getoption("artifacts_dir"))).resolve()
            timestamp = datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
            artifact_dir = artifacts_root / f"{_sanitize_nodeid(request.node.nodeid)}__{timestamp}"
            artifact_dir.mkdir(parents=True, exist_ok=True)
            artifact_dir.chmod(0o700)
            if setup_failure is not None:
                (artifact_dir / "setup-error.txt").write_text(
                    setup_failure, encoding="utf-8"
                )
            await capture_live_topology_artifacts(manager, artifact_dir, iteration_id)
        try:
            await manager.teardown()
        finally:
            if artifact_dir is not None and (
                test_failed or getattr(call_report, "failed", False)
            ):
                (artifact_dir / "cleanup.json").write_text(
                    json.dumps({"topology_teardown": "completed"}), encoding="utf-8"
                )


@pytest_asyncio.fixture
async def cli_pair(
    topology: TopologyManager,
    binaries: dict[str, str],
    request: pytest.FixtureRequest,
) -> AsyncIterator[dict[str, CliProcess]]:
    alice_namespace = topology.client_namespaces[0]
    bob_namespace = topology.client_namespaces[1]
    capture_audio_frame_indices = (
        request.node.get_closest_marker("capture_audio_frame_indices") is not None
    )

    alice = CliProcess(
        binary_path=binaries["cli"],
        namespace=alice_namespace,
        listen_port=0,
        bind_addresses=["0.0.0.0"],
        relay_url=topology.relay_url(alice_namespace),
        dns_endpoint=topology.dns_endpoint(alice_namespace),
        dns_origin_domain=topology.dns_origin_domain(alice_namespace),
        pkarr_relay=topology.pkarr_relay(alice_namespace),
        capture_audio_frame_indices=capture_audio_frame_indices,
    )
    bob = CliProcess(
        binary_path=binaries["cli"],
        namespace=bob_namespace,
        listen_port=0,
        bind_addresses=["0.0.0.0"],
        relay_url=topology.relay_url(bob_namespace),
        dns_endpoint=topology.dns_endpoint(bob_namespace),
        dns_origin_domain=topology.dns_origin_domain(bob_namespace),
        pkarr_relay=topology.pkarr_relay(bob_namespace),
        capture_audio_frame_indices=capture_audio_frame_indices,
    )
    test_failed = False
    try:
        await alice.start()
        await bob.start()

        await _set_identity_on_actor(alice, _generate_identity())
        await _set_identity_on_actor(bob, _generate_identity())
        alice_manager_start, bob_manager_start = await asyncio.gather(
            alice.send({"cmd": "start_manager", "args": {}}),
            bob.send({"cmd": "start_manager", "args": {}}),
        )
        assert alice_manager_start.get("ok") is True
        assert bob_manager_start.get("ok") is True
        await asyncio.gather(wait_for_relay_ready(alice), wait_for_relay_ready(bob))
        await wait_for_pkarr_published(
            [(alice, alice_namespace), (bob, bob_namespace)],
            topology,
            timeout=15.0,
        )

        yield {"alice": alice, "bob": bob}
    except Exception:
        test_failed = True
        raise
    finally:
        if test_failed:
            logs = await capture_dns_server_logs()
            artifacts_root = Path(str(request.config.getoption("artifacts_dir"))).resolve()
            timestamp = datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
            nodeid = _sanitize_nodeid(request.node.nodeid)
            artifact_dir = artifacts_root / f"{nodeid}__{timestamp}"
            artifact_dir.mkdir(parents=True, exist_ok=True)
            artifact_dir.chmod(0o700)
            (artifact_dir / "dns-server.log").write_text(logs, encoding="utf-8")
            await capture_live_topology_artifacts(topology, artifact_dir, "0")
        await bob.terminate()
        await alice.terminate()


@pytest_asyncio.fixture
async def room_cli_three(
    profile: NetworkProfile,
    worker_tag: str,
    binaries: dict[str, str],
) -> AsyncIterator[RoomCliGroup]:
    async for group in _room_cli_group_fixture(
        profile=profile,
        worker_tag=worker_tag,
        binaries=binaries,
        actor_names=ROOM_THREE_ACTORS,
    ):
        yield group


@pytest_asyncio.fixture
async def room_cli_four(
    profile: NetworkProfile,
    worker_tag: str,
    binaries: dict[str, str],
) -> AsyncIterator[RoomCliGroup]:
    async for group in _room_cli_group_fixture(
        profile=profile,
        worker_tag=worker_tag,
        binaries=binaries,
        actor_names=ROOM_FOUR_ACTORS,
    ):
        yield group


@pytest_asyncio.fixture
async def room_cli_twenty(
    profile: NetworkProfile,
    worker_tag: str,
    binaries: dict[str, str],
) -> AsyncIterator[RoomCliGroup]:
    async for group in _room_cli_group_fixture(
        profile=profile,
        worker_tag=worker_tag,
        binaries=binaries,
        actor_names=ROOM_TWENTY_ACTORS,
    ):
        yield group


async def _run_scenario(name: str, actors: dict[str, CliProcess]) -> None:
    runner = ScenarioRunner()
    scenario = runner.load(SCENARIOS_ROOT / name)
    variables: dict[str, str] = {}
    for actor_name, actor in actors.items():
        peer_id = actor.identity_peer_id
        if isinstance(peer_id, str):
            variables[f"{actor_name}.peer_id"] = peer_id
        identity = actor.identity
        if isinstance(identity, Identity):
            variables[f"{actor_name}.identity_key_b64"] = identity.secret_key_b64
    await runner.run(scenario, actors, initial_variables=variables)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_room_three_all_contacts_full_mesh(
    room_cli_three: RoomCliGroup,
    profile: NetworkProfile,
) -> None:
    _ = profile, room_cli_three.topology
    await _run_scenario("room_three_all_contacts.yaml", room_cli_three.actors)
    await wait_for_room_mesh_connected(room_cli_three.actors, timeout=120.0)


@pytest.mark.asyncio
@pytest.mark.xdist_group(name="room_twenty_partial_contacts_full_mesh")
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_room_twenty_partial_contacts_full_mesh(
    room_cli_twenty: RoomCliGroup,
    profile: NetworkProfile,
) -> None:
    _ = profile, room_cli_twenty.topology
    await _add_sparse_room_contacts(room_cli_twenty.actors)
    await _join_room_from_all(room_cli_twenty.actors, timeout=90.0)
    await wait_for_room_mesh_connected(
        room_cli_twenty.actors,
        timeout=300.0,
        stability_window=5.0,
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_room_four_mixed_contacts_full_mesh(
    room_cli_four: RoomCliGroup,
    profile: NetworkProfile,
) -> None:
    _ = profile, room_cli_four.topology
    await _run_scenario("room_four_mixed_contacts.yaml", room_cli_four.actors)
    await wait_for_room_mesh_connected(room_cli_four.actors, timeout=120.0)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_room_call_timeout_then_room_join_full_mesh(
    room_cli_three: RoomCliGroup,
    profile: NetworkProfile,
) -> None:
    _ = profile, room_cli_three.topology
    await _run_scenario("call_timeout_then_room_join.yaml", room_cli_three.actors)
    await wait_for_room_mesh_connected(room_cli_three.actors, timeout=120.0)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_room_end_releases_call_slot_for_rejoin(
    room_cli_three: RoomCliGroup,
    profile: NetworkProfile,
) -> None:
    _ = profile, room_cli_three.topology
    actors = room_cli_three.actors
    members = _room_member_peer_ids(actors)

    await _add_all_contacts(actors)
    await _join_room_from_all(actors, timeout=60.0)
    await wait_for_room_mesh_connected(actors, timeout=120.0)

    duplicate_response = await actors["alice"].send(
        {"cmd": "join_room", "args": {"members": members}}
    )
    _assert_ack_error_contains(duplicate_response, "A call is already active")

    await asyncio.gather(
        *(actor.send({"cmd": "end_call", "args": {}}) for actor in actors.values())
    )
    await asyncio.sleep(1.0)

    await _join_room_from_all(actors, timeout=60.0)
    await wait_for_room_mesh_connected(actors, timeout=120.0)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_room_peer_leave_and_rejoin(
    room_cli_four: RoomCliGroup,
    profile: NetworkProfile,
) -> None:
    _ = profile, room_cli_four.topology
    actors = room_cli_four.actors
    members = _room_member_peer_ids(actors)

    await _add_all_contacts(actors)
    await _join_room_from_all(actors, timeout=60.0)
    await wait_for_room_mesh_connected(actors, timeout=120.0)

    leaving = actors["carol"]
    leave_response = await leaving.send({"cmd": "end_call", "args": {}})
    assert leave_response.get("ok") is True, f"carol end_call failed: {leave_response}"

    remaining = {name: actor for name, actor in actors.items() if name != "carol"}
    await wait_for_room_mesh_connected(remaining, timeout=120.0)

    await _join_room_from_actor("carol", leaving, members, timeout=60.0)
    await wait_for_room_mesh_connected(actors, timeout=120.0)
    assert not _has_error_event(leaving.stdout_lines())


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_room_peer_hard_crash_relaunch_and_rejoin(
    room_cli_four: RoomCliGroup,
    profile: NetworkProfile,
) -> None:
    _ = profile
    actors = room_cli_four.actors

    await _add_all_contacts(actors)
    await _join_room_from_all(actors, timeout=60.0)
    await wait_for_room_mesh_connected(actors, timeout=120.0)

    crashed = actors["dave"]
    crashed_identity = getattr(crashed, "identity", None)
    if not isinstance(crashed_identity, Identity):
        raise AssertionError("dave identity missing before crash")

    await crashed.crash()
    remaining = {name: actor for name, actor in actors.items() if name != "dave"}
    await wait_for_room_mesh_connected(remaining, timeout=120.0)

    await crashed.restart()
    await _set_identity_on_actor(crashed, crashed_identity)
    restart_manager_response = await crashed.send({"cmd": "start_manager", "args": {}})
    assert restart_manager_response.get("ok") is True, (
        f"dave start_manager after crash failed: {restart_manager_response}"
    )
    await wait_for_relay_ready(crashed)
    await wait_for_pkarr_published(
        [(crashed, crashed.namespace)],
        room_cli_four.topology,
        timeout=30.0,
    )

    dave_peer_id = crashed.identity_peer_id
    if not isinstance(dave_peer_id, str):
        raise AssertionError("dave missing identity_peer_id after restart")
    await asyncio.gather(
        *(_add_contact(actor, "dave", dave_peer_id) for actor in remaining.values())
    )
    await _add_all_contacts({"dave": crashed, **remaining})

    members = _room_member_peer_ids(actors)
    await _join_room_from_actor("dave", crashed, members, timeout=60.0)
    await wait_for_room_mesh_connected(actors, timeout=120.0)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_smoke_ready(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("smoke_ready.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_smoke_session(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("smoke_session.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_session_one_sided_contact(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("session_one_sided_contact.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_session_simultaneous_dial(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("session_simultaneous_dial.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_call_simultaneous_dial(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("call_simultaneous_dial.yaml", cli_pair)

    alice = cli_pair["alice"]
    bob = cli_pair["bob"]

    alice_lines = alice.stdout_lines()
    bob_lines = bob.stdout_lines()

    def _call_states(messages: list[dict]) -> list[str]:
        states: list[str] = []
        for message in messages:
            if message.get("kind") != "event" or message.get("type") != "call_state":
                continue
            state = message.get("state")
            if isinstance(state, str):
                states.append(state)
            elif isinstance(state, dict) and len(state) == 1:
                only_key = next(iter(state.keys()))
                if isinstance(only_key, str):
                    states.append(only_key)
        return states

    alice_call_states = _call_states(alice_lines)
    bob_call_states = _call_states(bob_lines)
    assert (
        "Connected" in alice_call_states
    ), f"alice did not receive call_state Connected; observed {alice_call_states}"
    assert (
        "Connected" in bob_call_states
    ), f"bob did not receive call_state Connected; observed {bob_call_states}"


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_session_simultaneous_dial_then_call(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("session_simultaneous_dial_then_call.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_caller_cancel_during_glare_then_room(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("caller_cancel_during_glare_then_room.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_call_prompt_survives_session_restart(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    alice = cli_pair["alice"]
    bob = cli_pair["bob"]
    alice_peer_id = alice.identity_peer_id
    bob_peer_id = bob.identity_peer_id
    assert isinstance(alice_peer_id, str) and isinstance(bob_peer_id, str)

    await _add_contact(alice, "bob", bob_peer_id)
    await _add_contact(bob, "alice", alice_peer_id)
    for owner, peer in ((alice, bob_peer_id), (bob, alice_peer_id)):
        response = await owner.send({"cmd": "start_session", "args": {"contact_id": "bob" if owner is alice else "alice"}})
        assert response.get("ok") is True, f"start_session failed: {response}"
    await asyncio.gather(
        alice.expect_event(
            lambda event: event.get("type") == "session_status"
            and isinstance(event.get("status"), dict)
            and "Connected" in event["status"],
            timeout=45.0,
        ),
        bob.expect_event(
            lambda event: event.get("type") == "session_status"
            and isinstance(event.get("status"), dict)
            and "Connected" in event["status"],
            timeout=45.0,
        ),
    )

    response = await alice.send({"cmd": "start_call", "args": {"contact_id": "bob"}})
    assert response.get("ok") is True, f"start_call failed: {response}"
    original_prompt = await bob.expect_event(
        lambda event: event.get("type") == "accept_call_prompt",
        timeout=60.0,
    )
    original_request_id = original_prompt.get("request_id")

    alice_identity = alice.identity
    assert isinstance(alice_identity, Identity)
    await alice.restart()
    await _set_identity_on_actor(alice, alice_identity)
    restart_manager_response = await alice.send({"cmd": "start_manager", "args": {}})
    assert restart_manager_response.get("ok") is True, (
        f"start_manager after restart failed: {restart_manager_response}"
    )

    # The caller's process is gone, so Bob's original prompt must be cancelled
    # (directly or once the offer window elapses).
    try:
        await bob.expect_event(
            lambda event: event.get("type") == "accept_call_canceled"
            and event.get("request_id") == original_request_id,
            timeout=30.0,
        )
    except Exception as exc:
        raise AssertionError(
            "original prompt not cancelled after caller restart; "
            f"bob stdout tail: {bob.stdout_lines()[-30:]}; "
            f"bob stderr tail: {bob.stderr_lines()[-15:]}"
        ) from exc

    await _add_contact(alice, "bob", bob_peer_id)
    # Churn can drop a fresh session between its Connected emission and the
    # start_call lookup; retry the pair until the call lands.
    for _ in range(4):
        response = await alice.send({"cmd": "start_session", "args": {"contact_id": "bob"}})
        assert response.get("ok") is True, f"start_session after restart failed: {response}"
        await alice.expect_event(
            lambda event: event.get("type") == "session_status"
            and isinstance(event.get("status"), dict)
            and "Connected" in event["status"],
            timeout=45.0,
        )
        response = await alice.send({"cmd": "start_call", "args": {"contact_id": "bob"}})
        if response.get("ok") is True:
            break
    else:
        raise AssertionError("start_call after restart never landed on a live session")

    # Session churn can cancel a raised prompt before the accept lands; accept
    # the first prompt that is still open, re-issuing the call when Alice's
    # negotiation has already terminalized.
    last_error: Exception | None = None
    for _ in range(4):
        try:
            prompt = await bob.expect_event(
                lambda event: event.get("type") == "accept_call_prompt",
                timeout=45.0,
            )
        except Exception as exc:
            last_error = exc
            await alice.send({"cmd": "start_session", "args": {"contact_id": "bob"}})
            response = await alice.send({"cmd": "start_call", "args": {"contact_id": "bob"}})
            continue
        accept_response = await bob.send(
            {
                "cmd": "accept_call",
                "args": {"request_id": prompt.get("request_id"), "accept": True},
            }
        )
        if accept_response.get("ok") is True:
            break
    else:
        raise AssertionError(
            f"no accept_call succeeded across raised prompts: {last_error}; "
            f"bob stdout tail: {bob.stdout_lines()[-30:]}; "
            f"bob stderr tail: {bob.stderr_lines()[-30:]}; "
            f"alice stdout tail: {alice.stdout_lines()[-15:]}"
        )

    await asyncio.gather(
        alice.expect_event(lambda event: _is_call_state(event, "Connected"), timeout=20.0),
        bob.expect_event(lambda event: _is_call_state(event, "Connected"), timeout=20.0),
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_call_hello_ack_timeout(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("call_hello_ack_timeout.yaml", cli_pair)

    alice = cli_pair["alice"]
    alice_lines = alice.stdout_lines()

    def _call_states(messages: list[dict]) -> list[str]:
        states: list[str] = []
        for message in messages:
            if message.get("kind") != "event" or message.get("type") != "call_state":
                continue
            state = message.get("state")
            if isinstance(state, str):
                states.append(state)
            elif isinstance(state, dict) and len(state) == 1:
                only_key = next(iter(state.keys()))
                if isinstance(only_key, str):
                    states.append(only_key)
        return states

    alice_call_states = _call_states(alice_lines)
    assert (
        "CallEnded" in alice_call_states
    ), f"alice did not receive call_state CallEnded; observed {alice_call_states}"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile", [REPEATED_CALL_PROFILE], ids=lambda profile: profile.name
)
async def test_call_repeated_without_restart_keeps_remote_audio(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    await _run_scenario("call_repeated_without_restart.yaml", cli_pair)

    alice = cli_pair["alice"]
    bob = cli_pair["bob"]

    await _start_call_and_wait_connected(
        alice,
        bob,
        caller_contact_id="bob",
        callee_contact_id="alice",
    )

    first_call_bob_stats = await _collect_statistics_window(bob, duration=3.0)
    _assert_no_sustained_full_loss("bob during first call", first_call_bob_stats)

    # Let the audio sequence climb so the bounded datagram queue holds enough
    # stale frames to keep bob fully dark past the 20-stat / 2 s threshold
    # during the second call's handshake.
    await asyncio.sleep(5.0)

    end_response = await bob.send({"cmd": "end_call", "args": {}})
    assert end_response.get("ok") is True, f"bob end_call failed: {end_response}"

    # bob must start a new call soon after the previous ends
    # sleep serves this purpose okay
    await asyncio.sleep(0.5)

    await _start_call_and_wait_connected(
        alice,
        bob,
        caller_contact_id="bob",
        callee_contact_id="alice",
    )

    second_call_bob_stats = await _collect_statistics_window(bob, duration=4.0)
    _assert_no_sustained_full_loss("bob during second call", second_call_bob_stats)
    max_second_call_output = _max_stat_value(second_call_bob_stats, "output_level")
    assert max_second_call_output > 0.0, (
        "bob did not observe nonzero output_level during second call; "
        f"max_output_level={max_second_call_output}, stats={second_call_bob_stats}"
    )


@pytest.mark.asyncio
@pytest.mark.capture_audio_frame_indices
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_call_drain_audio_frame_indices_strictly_increasing(
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    alice = cli_pair["alice"]
    bob = cli_pair["bob"]

    alice_peer_id = alice.identity_peer_id
    bob_peer_id = bob.identity_peer_id
    if not isinstance(alice_peer_id, str) or not isinstance(bob_peer_id, str):
        raise AssertionError("missing CLI identity peer ID")
    await asyncio.gather(
        _add_contact(alice, "bob", bob_peer_id),
        _add_contact(bob, "alice", alice_peer_id),
    )

    start_responses = await asyncio.gather(
        alice.send({"cmd": "start_session", "args": {"contact_id": "bob"}}),
        bob.send({"cmd": "start_session", "args": {"contact_id": "alice"}}),
    )
    assert all(response.get("ok") is True for response in start_responses)
    await asyncio.gather(
        alice.expect_event(
            lambda event: event.get("type") == "session_status"
            and _status_name(event.get("status")) == "Connected",
            timeout=20.0,
        ),
        bob.expect_event(
            lambda event: event.get("type") == "session_status"
            and _status_name(event.get("status")) == "Connected",
            timeout=20.0,
        ),
    )

    await _start_call_and_wait_connected(
        alice,
        bob,
        caller_contact_id="bob",
        callee_contact_id="alice",
    )

    await asyncio.sleep(3.0)

    response = await bob.send({"cmd": "drain_audio_frame_indices", "args": {}})
    data = response.get("data")
    indices = data.get("indices") if isinstance(data, dict) else None
    assert response.get("kind") == "result", (
        f"profile={profile.name}, observed_indices={indices}, response={response}"
    )

    assert isinstance(indices, list), (
        f"profile={profile.name} drain_audio_frame_indices returned non-list indices: {response}"
    )
    assert 1 < len(indices) <= 512, (
        f"profile={profile.name}, observed_indices={indices}"
    )
    assert all(isinstance(index, int) for index in indices), (
        f"profile={profile.name}, observed_indices={indices}"
    )
    assert all(right > left for left, right in zip(indices, indices[1:])), (
        f"profile={profile.name}, observed_indices={indices}"
    )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile",
    NETWORK_PROFILES,
    ids=lambda profile: profile.name,
)
async def test_session_client_disappears_and_reappears(
    topology: TopologyManager,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, profile
    alice = cli_pair["alice"]
    bob = cli_pair["bob"]

    def _status_name(status: object) -> str | None:
        if isinstance(status, str):
            return status
        if isinstance(status, dict) and len(status) == 1:
            only_key = next(iter(status.keys()))
            if isinstance(only_key, str):
                return only_key
        return None

    def _is_connected_event(event: dict) -> bool:
        if event.get("type") != "session_status":
            return False
        return _status_name(event.get("status")) == "Connected"

    alice_ready = await alice.expect_event(
        lambda event: event.get("type") == "ready", timeout=10.0
    )
    bob_ready = await bob.expect_event(
        lambda event: event.get("type") == "ready", timeout=10.0
    )
    assert alice_ready.get("type") == "ready"
    assert bob_ready.get("type") == "ready"

    alice_add_contact = await alice.send(
        {
            "cmd": "add_contact",
            "args": {"contact_id": "bob", "peer_id": bob.identity_peer_id},
        }
    )
    bob_add_contact = await bob.send(
        {
            "cmd": "add_contact",
            "args": {"contact_id": "alice", "peer_id": alice.identity_peer_id},
        }
    )
    assert alice_add_contact.get("ok") is True
    assert bob_add_contact.get("ok") is True

    start_response = await alice.send(
        {"cmd": "start_session", "args": {"contact_id": "bob"}}
    )
    assert start_response.get("ok") is True

    alice_connecting = await alice.expect_event(
        lambda event: event.get("type") == "session_status"
        and _status_name(event.get("status")) in {"Connecting", "Connected"},
        timeout=10.0,
    )
    assert _status_name(alice_connecting.get("status")) in {"Connecting", "Connected"}

    alice_stdout_before_restart = len(alice.stdout_lines())
    await bob.terminate()
    await asyncio.sleep(2.0)
    await bob.restart()
    bob_identity = getattr(bob, "identity", None)
    if not isinstance(bob_identity, Identity):
        raise AssertionError("bob identity missing before restart")
    await _set_identity_on_actor(bob, bob_identity)

    bob_start_manager = await bob.send({"cmd": "start_manager", "args": {}})
    assert bob_start_manager.get("ok") is True
    bob_add_contact_after_restart = await bob.send(
        {
            "cmd": "add_contact",
            "args": {"contact_id": "alice", "peer_id": alice.identity_peer_id},
        }
    )
    assert bob_add_contact_after_restart.get("ok") is True

    # simulate frontend starting sessions on launch
    start_response = await bob.send(
        {"cmd": "start_session", "args": {"contact_id": "alice"}}
    )
    assert start_response.get("ok") is True

    bob_session = await bob.expect_event(_is_connected_event, timeout=30.0)
    assert _status_name(bob_session.get("status")) == "Connected"

    alice_connected = await alice.expect_event(_is_connected_event, timeout=30.0)
    assert alice_connected.get("peer") == bob.identity_peer_id

    def _session_statuses(messages: list[dict]) -> list[str]:
        statuses: list[str] = []
        for message in messages:
            if message.get("kind") != "event" or message.get("type") != "session_status":
                continue
            status_name = _status_name(message.get("status"))
            if status_name:
                statuses.append(status_name)
        return statuses

    alice_after_restart = alice.stdout_lines()[alice_stdout_before_restart:]
    bob_after_restart = bob.stdout_lines()
    alice_statuses = _session_statuses(alice_after_restart)
    bob_statuses = _session_statuses(bob_after_restart)
    assert alice_statuses, "alice emitted no session_status events after restart"
    assert bob_statuses, "bob emitted no session_status events after restart"
    assert "Connected" in alice_statuses
    assert "Connected" in bob_statuses
    assert "Unknown" not in alice_statuses
    assert "Unknown" not in bob_statuses

    def _has_error_event(messages: list[dict]) -> bool:
        for message in messages:
            if message.get("kind") != "event":
                continue
            event_type = message.get("type")
            if event_type in {"Error", "error"}:
                return True
        return False

    assert not _has_error_event(alice_after_restart)
    assert not _has_error_event(bob_after_restart)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", [NETWORK_PROFILES[0]], ids=lambda profile: profile.name)
async def test_start_manager_ack_precedes_active_event(
    topology: TopologyManager,
    binaries: dict[str, str],
    profile: NetworkProfile,
) -> None:
    _ = profile
    namespace = topology.client_namespaces[0]

    actor = CliProcess(
        binary_path=binaries["cli"],
        namespace=namespace,
        listen_port=0,
        bind_addresses=["0.0.0.0"],
        relay_url=topology.relay_url(namespace),
        dns_endpoint=topology.dns_endpoint(namespace),
        dns_origin_domain=topology.dns_origin_domain(namespace),
        pkarr_relay=topology.pkarr_relay(namespace),
    )

    try:
        await actor.start()
        await _set_identity_on_actor(actor, _generate_identity())

        loop = asyncio.get_running_loop()
        ack_start = loop.time()
        ack = await actor.send({"cmd": "start_manager", "args": {}})
        ack_elapsed = loop.time() - ack_start

        assert ack.get("ok") is True, f"start_manager ack failed: {ack}"

        active_event = await actor.expect_event(
            lambda event: _manager_state_from_event(event) == "Active",
            timeout=30.0,
        )
        active_elapsed = loop.time() - ack_start

        assert active_elapsed > ack_elapsed, (
            "manager_active Active was observed before the start_manager ack, "
            "indicating start_manager blocked until Active. "
            f"ack_elapsed={ack_elapsed:.3f}s, active_elapsed={active_elapsed:.3f}s"
        )
        assert ack_elapsed < 1.0, (
            f"start_manager ack took {ack_elapsed:.3f}s — the manager spawn "
            "should return near-instantly; the wait-for-active path appears to "
            "have been restored"
        )

        _ = active_event
    finally:
        await actor.terminate()
