from __future__ import annotations

import asyncio
import base64
from pathlib import Path

import pytest
import pytest_asyncio

from harness.process import CliProcess, RelayProcess
from harness.scenario import ScenarioRunner
from harness.topology import NetworkProfile, TopologyManager


SCENARIOS_ROOT = Path(__file__).resolve().parents[1] / "scenarios"
_BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
_TEST_IDENTITY_KEYS_B64 = (
    "CAESQBWzTWw8yk7ApiUqDgYLm2XvY5tPcRbpLEZKlmLo108QfjxIYLTx1jCi1PoNTRguryhS+EyLw+fELYfAM2Rnk/A=",
    "CAESQK30hW7xvWg87VbBv3c0x0VdBiK53TAW8oVQUSrhKwh+tkfQ1axxMb3Yv0wRTGlj9imiBq1DukErpytZsRD88tE=",
)

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
]


def _base58btc_encode(raw: bytes) -> str:
    value = int.from_bytes(raw, "big")
    encoded = ""
    while value > 0:
        value, remainder = divmod(value, 58)
        encoded = _BASE58_ALPHABET[remainder] + encoded

    leading_zeroes = len(raw) - len(raw.lstrip(b"\x00"))
    return ("1" * leading_zeroes) + (encoded or "1")


def _peer_id_from_key_b64(key_b64: str) -> str:
    key_bytes = base64.b64decode(key_b64)
    if len(key_bytes) != 68 or key_bytes[:4] != b"\x08\x01\x12\x40":
        raise AssertionError("unexpected protobuf-encoded ed25519 private key format")

    public_key = key_bytes[36:68]
    protobuf_public_key = b"\x08\x01\x12\x20" + public_key
    identity_multihash = b"\x00\x24" + protobuf_public_key
    return _base58btc_encode(identity_multihash)


async def _set_identity(actor: CliProcess, key_b64: str) -> str:
    response = await actor.send({"cmd": "set_identity", "args": {"key_b64": key_b64}})
    if not response.get("ok"):
        raise AssertionError(f"set_identity failed: {response}")
    return _peer_id_from_key_b64(key_b64)


async def _set_identity_on_actor(actor: CliProcess, key_b64: str) -> str:
    peer_id = await _set_identity(actor, key_b64)
    actor.identity_peer_id = peer_id
    return peer_id


async def wait_for_relay_ready(actor: CliProcess, timeout: float = 30.0) -> dict:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while loop.time() < deadline:
        for message in actor.stdout_lines():
            if (
                message.get("kind") == "event"
                and message.get("type") == "manager_active"
                and message.get("active") is True
            ):
                return message
        await asyncio.sleep(0.05)

    raise AssertionError(
        "timed out waiting for manager_active relay readiness event; "
        f"stdout transcript: {actor.stdout_lines()}"
    )


@pytest_asyncio.fixture
async def topology(profile: NetworkProfile) -> TopologyManager:
    manager = TopologyManager()
    try:
        await manager.setup(num_clients=2, profile=profile)
        yield manager
    finally:
        await manager.teardown()


@pytest_asyncio.fixture
async def relay(topology: TopologyManager, binaries: dict[str, str]) -> RelayProcess:
    proc = RelayProcess(
        binary_path=binaries["relay"],
        namespace=topology.relay_namespace,
        listen_addr="0.0.0.0:40142",
    )
    await proc.start()
    try:
        yield proc
    finally:
        await proc.terminate()


@pytest_asyncio.fixture
async def cli_pair(
    topology: TopologyManager,
    relay: RelayProcess,
    binaries: dict[str, str],
) -> dict[str, CliProcess]:
    if not relay.peer_id:
        raise AssertionError("relay peer id missing")

    alice = CliProcess(
        binary_path=binaries["cli"],
        namespace="ns-cli-0",
        relay_addr=topology.relay_addr("ns-cli-0"),
        relay_peer=relay.peer_id,
    )
    bob = CliProcess(
        binary_path=binaries["cli"],
        namespace="ns-cli-1",
        relay_addr=topology.relay_addr("ns-cli-1"),
        relay_peer=relay.peer_id,
    )
    await alice.start()
    await bob.start()

    await _set_identity_on_actor(alice, _TEST_IDENTITY_KEYS_B64[0])
    await _set_identity_on_actor(bob, _TEST_IDENTITY_KEYS_B64[1])
    alice_manager_start, bob_manager_start = await asyncio.gather(
        alice.send({"cmd": "start_manager", "args": {}}),
        bob.send({"cmd": "start_manager", "args": {}}),
    )
    assert alice_manager_start.get("ok") is True
    assert bob_manager_start.get("ok") is True
    await asyncio.gather(wait_for_relay_ready(alice), wait_for_relay_ready(bob))
    try:
        yield {"alice": alice, "bob": bob}
    finally:
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
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, relay, profile
    await _run_scenario("smoke_ready.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_smoke_session(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, relay, profile
    await _run_scenario("smoke_session.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_session_one_sided_contact(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, relay, profile
    await _run_scenario("session_one_sided_contact.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_session_simultaneous_dial(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, relay, profile
    await _run_scenario("session_simultaneous_dial.yaml", cli_pair)


@pytest.mark.asyncio
@pytest.mark.parametrize("profile", NETWORK_PROFILES, ids=lambda profile: profile.name)
async def test_call_simultaneous_dial(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, relay, profile
    await _run_scenario("call_simultaneous_dial.yaml", cli_pair)

    alice = cli_pair["alice"]
    bob = cli_pair["bob"]

    def _has_accept_call_prompt(messages: list[dict]) -> bool:
        for message in messages:
            if message.get("kind") != "event":
                continue
            if message.get("type") == "accept_call_prompt":
                return True
        return False

    alice_lines = alice.stdout_lines()
    bob_lines = bob.stdout_lines()
    assert not _has_accept_call_prompt(
        alice_lines
    ), "alice emitted undesired accept_call_prompt during simultaneous dial"
    assert not _has_accept_call_prompt(
        bob_lines
    ), "bob emitted undesired accept_call_prompt during simultaneous dial"

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
@pytest.mark.parametrize(
    "profile",
    NETWORK_PROFILES,
    ids=lambda profile: profile.name,
)
async def test_session_client_disappears_and_reappears(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, relay, profile
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
    await _set_identity_on_actor(bob, _TEST_IDENTITY_KEYS_B64[1])

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
