from __future__ import annotations

import asyncio
import base64
import re
import urllib.error
import urllib.request
from dataclasses import dataclass
from datetime import datetime
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
class TestIdentity:
    secret_key_b64: str
    peer_id_hex: str

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


def _generate_identity() -> TestIdentity:
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
    return TestIdentity(
        secret_key_b64=base64.b64encode(secret_key).decode("ascii"),
        peer_id_hex=public_key.hex(),
    )


async def _set_identity(actor: CliProcess, identity: TestIdentity) -> str:
    response = await actor.send(
        {
            "cmd": "set_identity",
            "args": {"key_b64": identity.secret_key_b64},
        }
    )
    if not response.get("ok"):
        raise AssertionError(f"set_identity failed: {response}")
    return identity.peer_id_hex


async def _set_identity_on_actor(actor: CliProcess, identity: TestIdentity) -> str:
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
    return zbase32.encode(bytes.fromhex(peer_id_hex))


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
    process = await asyncio.create_subprocess_exec(
        "docker",
        "logs",
        "iroh-dns-server",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=timeout)
    except asyncio.TimeoutError:
        process.kill()
        await process.wait()
        return f"docker logs iroh-dns-server timed out after {timeout}s"

    logs = stdout.decode("utf-8", errors="replace")
    errors = stderr.decode("utf-8", errors="replace")
    if errors.strip():
        return f"{logs}\n--- stderr ---\n{errors}"
    return logs


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


@pytest_asyncio.fixture
async def topology(
    profile: NetworkProfile,
    worker_tag: str,
) -> TopologyManager:
    manager = TopologyManager(worker_id=worker_tag)
    try:
        await manager.setup(num_clients=2, profile=profile)
        if not manager.client_namespaces:
            pytest.skip(
                "topology setup did not create client namespaces; "
                "network namespace privileges (CAP_NET_ADMIN / ip netns) are required"
            )
        yield manager
    finally:
        await manager.teardown()


@pytest_asyncio.fixture
async def cli_pair(
    topology: TopologyManager,
    binaries: dict[str, str],
    request: pytest.FixtureRequest,
) -> dict[str, CliProcess]:
    alice_namespace = topology.client_namespaces[0]
    bob_namespace = topology.client_namespaces[1]

    alice = CliProcess(
        binary_path=binaries["cli"],
        namespace=alice_namespace,
        listen_port=0,
        bind_addresses=["0.0.0.0"],
        relay_url=topology.relay_url(alice_namespace),
        dns_endpoint=topology.dns_endpoint(alice_namespace),
        pkarr_relay=topology.pkarr_relay(alice_namespace),
    )
    bob = CliProcess(
        binary_path=binaries["cli"],
        namespace=bob_namespace,
        listen_port=0,
        bind_addresses=["0.0.0.0"],
        relay_url=topology.relay_url(bob_namespace),
        dns_endpoint=topology.dns_endpoint(bob_namespace),
        pkarr_relay=topology.pkarr_relay(bob_namespace),
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
            (artifact_dir / "dns-server.log").write_text(logs, encoding="utf-8")
        await bob.terminate()
        await alice.terminate()


async def _run_scenario(name: str, actors: dict[str, CliProcess]) -> None:
    runner = ScenarioRunner()
    scenario = runner.load(SCENARIOS_ROOT / name)
    variables: dict[str, str] = {}
    for actor_name, actor in actors.items():
        peer_id = actor.identity_peer_id
        if isinstance(peer_id, str):
            variables[f"{actor_name}.peer_id"] = peer_id
    await runner.run(scenario, actors, initial_variables=variables)


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
    if not isinstance(bob_identity, TestIdentity):
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
