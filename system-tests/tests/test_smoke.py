from __future__ import annotations

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

    alice_peer_id = await _set_identity(alice, _TEST_IDENTITY_KEYS_B64[0])
    bob_peer_id = await _set_identity(bob, _TEST_IDENTITY_KEYS_B64[1])
    alice.identity_peer_id = alice_peer_id
    bob.identity_peer_id = bob_peer_id
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
async def test_smoke_manager(
    topology: TopologyManager,
    relay: RelayProcess,
    cli_pair: dict[str, CliProcess],
    profile: NetworkProfile,
) -> None:
    _ = topology, relay, profile
    await _run_scenario("smoke_manager.yaml", cli_pair)


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
