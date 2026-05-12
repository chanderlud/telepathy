from __future__ import annotations

import asyncio
import hashlib
import random
from dataclasses import dataclass
from pathlib import Path

import pytest
import pytest_asyncio

from harness.process import CliProcess, RelayProcess, peer_id_from_identity_key_b64
from harness.scenario import ScenarioRunner
from harness.topology import NetworkProfile, TopologyManager


SCENARIOS_ROOT = Path(__file__).resolve().parents[1] / "scenarios"
_TEST_IDENTITY_KEYS_B64 = (
    "CAESQBWzTWw8yk7ApiUqDgYLm2XvY5tPcRbpLEZKlmLo108QfjxIYLTx1jCi1PoNTRguryhS+EyLw+fELYfAM2Rnk/A=",
    "CAESQK30hW7xvWg87VbBv3c0x0VdBiK53TAW8oVQUSrhKwh+tkfQ1axxMb3Yv0wRTGlj9imiBq1DukErpytZsRD88tE=",
)


@dataclass(frozen=True)
class ProfileTemplate:
    name: str
    delay_ms_range: tuple[int, int]
    jitter_ms_range: tuple[int, int]
    loss_pct_range: tuple[float, float]
    burst_loss: bool


PROFILE_TEMPLATES = [
    ProfileTemplate("clean", (0, 0), (0, 0), (0.0, 0.0), False),
    ProfileTemplate("wan", (40, 120), (5, 30), (0.0, 0.5), False),
    ProfileTemplate("bad_mobile", (150, 350), (50, 150), (1.0, 5.0), True),
    ProfileTemplate("satellite", (250, 500), (100, 200), (4.0, 10.0), True),
]


def _profile_for_iteration(
    template: ProfileTemplate, iteration_id: str, worker_tag: str
) -> NetworkProfile:
    """Derive a concrete NetworkProfile for one parametrized iteration.

    Seeding uses BLAKE2b over template name, iteration id, and worker tag so the
    chosen delay/jitter/loss is stable across processes and PYTHONHASHSEED values
    (artifacts can reproduce the same profile from ``profile.seed``).
    """
    seed_material = f"{template.name}|{iteration_id}|{worker_tag}".encode()
    seed = int.from_bytes(
        hashlib.blake2b(seed_material, digest_size=8).digest(), "big"
    )
    rng = random.Random(seed)
    d_lo, d_hi = template.delay_ms_range
    j_lo, j_hi = template.jitter_ms_range
    l_lo, l_hi = template.loss_pct_range
    delay_ms = rng.randint(min(d_lo, d_hi), max(d_lo, d_hi))
    jitter_ms = rng.randint(min(j_lo, j_hi), max(j_lo, j_hi))
    loss_pct = rng.uniform(l_lo, l_hi)
    return NetworkProfile(
        name=f"{template.name}-{iteration_id}",
        delay_ms=delay_ms,
        jitter_ms=jitter_ms,
        loss_pct=round(loss_pct, 2),
        burst_loss=template.burst_loss,
        seed=seed,
    )


@pytest.fixture
def profile(
    profile_template: ProfileTemplate,
    iteration_id: str,
    worker_tag: str,
) -> NetworkProfile:
    return _profile_for_iteration(profile_template, iteration_id, worker_tag)


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


def _session_statuses(messages: list[dict]) -> list[str]:
    statuses: list[str] = []
    for message in messages:
        if message.get("kind") != "event" or message.get("type") != "session_status":
            continue
        status_name = _status_name(message.get("status"))
        if status_name:
            statuses.append(status_name)
    return statuses


def _has_error_event(messages: list[dict]) -> bool:
    for message in messages:
        if message.get("kind") != "event":
            continue
        event_type = message.get("type")
        if event_type in {"Error", "error"}:
            return True
    return False


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


async def _set_identity(actor: CliProcess, key_b64: str) -> str:
    response = await actor.send({"cmd": "set_identity", "args": {"key_b64": key_b64}})
    if not response.get("ok"):
        raise AssertionError(f"set_identity failed: {response}")
    return peer_id_from_identity_key_b64(key_b64)


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

    transcript = "\n".join(str(message) for message in actor.stdout_lines())
    raise AssertionError(
        "manager_active event not observed within timeout.\n"
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
        yield manager
    finally:
        await manager.teardown()


@pytest_asyncio.fixture
async def relay(topology: TopologyManager, binaries: dict[str, str]) -> RelayProcess:
    proc = RelayProcess(
        binary_path=binaries["relay"],
        namespace=topology.relay_namespace,
        listen_addr=f"0.0.0.0:{topology.listen_port}",
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

    alice_namespace = topology.client_namespaces[0]
    bob_namespace = topology.client_namespaces[1]

    alice = CliProcess(
        binary_path=binaries["cli"],
        namespace=alice_namespace,
        relay_addr=topology.relay_addr(alice_namespace),
        relay_peer=relay.peer_id,
    )
    bob = CliProcess(
        binary_path=binaries["cli"],
        namespace=bob_namespace,
        relay_addr=topology.relay_addr(bob_namespace),
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
    await asyncio.sleep(1)  # TODO implement a better way to wait for peers to connect to relay
    try:
        yield {"alice": alice, "bob": bob}
    finally:
        await bob.terminate()
        await alice.terminate()


def _actor_namespace_map(
    topology: TopologyManager, actors: dict[str, CliProcess]
) -> dict[str, str]:
    namespaces = list(topology.client_namespaces)
    mapping: dict[str, str] = {}
    if "alice" in actors and len(namespaces) > 0:
        mapping["alice"] = namespaces[0]
    if "bob" in actors and len(namespaces) > 1:
        mapping["bob"] = namespaces[1]
    return mapping


async def _run_scenario(
    name: str,
    actors: dict[str, CliProcess],
    topology: TopologyManager,
) -> None:
    runner = ScenarioRunner(
        topology=topology,
        actor_namespaces=_actor_namespace_map(topology, actors),
    )
    scenario = runner.load(SCENARIOS_ROOT / name)
    variables: dict[str, object] = {}
    identity_keys = {"alice": _TEST_IDENTITY_KEYS_B64[0], "bob": _TEST_IDENTITY_KEYS_B64[1]}
    for actor_name, actor in actors.items():
        peer_id = actor.identity_peer_id
        if isinstance(peer_id, str):
            variables[f"{actor_name}.peer_id"] = peer_id
        key_b64 = identity_keys.get(actor_name)
        if isinstance(key_b64, str):
            variables[f"{actor_name}.identity_key_b64"] = key_b64
    await runner.run(scenario, actors, initial_variables=variables)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_smoke_ready(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    await _run_scenario("smoke_ready.yaml", cli_pair, topology)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_smoke_session(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    await _run_scenario("smoke_session.yaml", cli_pair, topology)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_session_one_sided_contact(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    await _run_scenario("session_one_sided_contact.yaml", cli_pair, topology)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_session_simultaneous_dial(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    await _run_scenario("session_simultaneous_dial.yaml", cli_pair, topology)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_call_simultaneous_dial(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    await _run_scenario("call_simultaneous_dial.yaml", cli_pair, topology)

    alice = cli_pair["alice"]
    bob = cli_pair["bob"]

    alice_lines = alice.stdout_lines()
    bob_lines = bob.stdout_lines()

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
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_session_client_disappears_and_reappears(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    alice = cli_pair["alice"]
    bob = cli_pair["bob"]

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

    assert not _has_error_event(alice_after_restart)
    assert not _has_error_event(bob_after_restart)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_session_after_peer_hard_crash(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    await _run_scenario("session_after_peer_hard_crash.yaml", cli_pair, topology)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_session_handshake_lag_then_retry(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    await _run_scenario("session_handshake_lag_then_retry.yaml", cli_pair, topology)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "profile_template",
    PROFILE_TEMPLATES,
    ids=lambda template: template.name,
)
async def test_session_and_call_survive_link_flap(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
    profile_template: ProfileTemplate,
) -> None:
    _ = topology, relay, profile, profile_template
    alice = cli_pair["alice"]
    bob = cli_pair["bob"]
    bob_namespace = topology.client_namespaces[1]

    await alice.expect_event(lambda e: e.get("type") == "ready", timeout=10.0)
    await bob.expect_event(lambda e: e.get("type") == "ready", timeout=10.0)

    assert (
        await alice.send(
            {
                "cmd": "add_contact",
                "args": {"contact_id": "bob", "peer_id": bob.identity_peer_id},
            }
        )
    ).get("ok") is True
    assert (
        await bob.send(
            {
                "cmd": "add_contact",
                "args": {"contact_id": "alice", "peer_id": alice.identity_peer_id},
            }
        )
    ).get("ok") is True

    assert (
        await alice.send({"cmd": "start_session", "args": {"contact_id": "bob"}})
    ).get("ok") is True

    await asyncio.gather(
        alice.expect_event(_is_connected_event, timeout=45.0),
        bob.expect_event(_is_connected_event, timeout=45.0),
    )

    bob_stdout_before_call = len(bob.stdout_lines())
    assert (
        await alice.send({"cmd": "start_call", "args": {"contact_id": "bob"}})
    ).get("ok") is True

    async def _bob_handle_call_prompt() -> None:
        # Single-dial path: require an explicit prompt (dual-dial tests use a
        # separate helper if call_state can arrive without a prompt).
        def _prompt_after_call_start(event: dict) -> bool:
            if event.get("type") != "accept_call_prompt":
                return False
            return event in bob.stdout_lines()[bob_stdout_before_call:]

        ev = await bob.expect_event(_prompt_after_call_start, timeout=30.0)
        rid = ev.get("request_id")
        assert isinstance(rid, str)
        assert (
            await bob.send(
                {"cmd": "accept_call", "args": {"request_id": rid, "accept": True}}
            )
        ).get("ok") is True

    await _bob_handle_call_prompt()

    await asyncio.gather(
        alice.expect_event(
            lambda e: e.get("type") == "call_state"
            and "Connected" in _call_states([e]),
            timeout=30.0,
        ),
        bob.expect_event(
            lambda e: e.get("type") == "call_state"
            and "Connected" in _call_states([e]),
            timeout=30.0,
        ),
    )

    alice_idx_before = len(alice.stdout_lines())
    bob_idx_before = len(bob.stdout_lines())
    await topology.restart_link(bob_namespace, down_seconds=1.0)

    recovery_deadline = asyncio.get_running_loop().time() + 30.0

    async def _wait_post_flap_session_connected(actor: CliProcess, start: int) -> None:
        while asyncio.get_running_loop().time() < recovery_deadline:
            lines = actor.stdout_lines()[start:]
            for msg in lines:
                if msg.get("kind") == "event" and _is_connected_event(msg):
                    return
            await asyncio.sleep(0.05)
        raise AssertionError(f"{actor} did not observe session_status Connected after flap")

    async def _wait_post_flap_call_connected(actor: CliProcess, start: int) -> None:
        while asyncio.get_running_loop().time() < recovery_deadline:
            lines = actor.stdout_lines()[start:]
            for msg in lines:
                if (
                    msg.get("kind") == "event"
                    and msg.get("type") == "call_state"
                    and "Connected" in _call_states([msg])
                ):
                    return
            await asyncio.sleep(0.05)
        raise AssertionError(f"{actor} did not observe call_state Connected after flap")

    await asyncio.gather(
        _wait_post_flap_session_connected(alice, alice_idx_before),
        _wait_post_flap_session_connected(bob, bob_idx_before),
        _wait_post_flap_call_connected(alice, alice_idx_before),
        _wait_post_flap_call_connected(bob, bob_idx_before),
    )

    alice_after = alice.stdout_lines()[alice_idx_before:]
    bob_after = bob.stdout_lines()[bob_idx_before:]
    assert not _has_error_event(alice_after)
    assert not _has_error_event(bob_after)

    assert (await alice.send({"cmd": "end_call", "args": {}})).get("ok") is True

    await asyncio.gather(
        alice.expect_event(
            lambda e: e.get("type") == "call_state" and "CallEnded" in _call_states([e]),
            timeout=30.0,
        ),
        bob.expect_event(
            lambda e: e.get("type") == "call_state" and "CallEnded" in _call_states([e]),
            timeout=30.0,
        ),
    )

    steady_start_alice = len(alice.stdout_lines())
    steady_start_bob = len(bob.stdout_lines())
    await topology.flap_link(
        bob_namespace, count=3, down_seconds=0.4, up_seconds=0.4
    )

    recovery2 = asyncio.get_running_loop().time() + 30.0

    async def _wait_steady_connected(actor: CliProcess, start: int) -> None:
        while asyncio.get_running_loop().time() < recovery2:
            for msg in actor.stdout_lines()[start:]:
                if msg.get("kind") == "event" and _is_connected_event(msg):
                    if msg.get("peer") == (
                        bob.identity_peer_id if actor is alice else alice.identity_peer_id
                    ):
                        return
            await asyncio.sleep(0.05)
        raise AssertionError("session did not return to Connected after steady-state flap")

    await asyncio.gather(
        _wait_steady_connected(alice, steady_start_alice),
        _wait_steady_connected(bob, steady_start_bob),
    )
