from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from random import Random
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import pytest

from harness.process import CliProcess
from harness.topology import TopologyManager


SYSTEM_TEST_ROOT = Path(__file__).resolve().parent
REPO_ROOT = SYSTEM_TEST_ROOT.parent
BUILD_SCRIPT = SYSTEM_TEST_ROOT / "build.sh"
GEN_CERTS_SCRIPT = SYSTEM_TEST_ROOT / "relay" / "gen-certs.sh"
RELAY_CERTS_DIR = SYSTEM_TEST_ROOT / "relay" / "certs"
RELAY_CERT_FILES = (
    RELAY_CERTS_DIR / "cert.pem",
    RELAY_CERTS_DIR / "cert.key.pem",
)
RUST_TARGET = REPO_ROOT / "rust" / "target" / "debug"

if str(SYSTEM_TEST_ROOT) not in sys.path:
    sys.path.insert(0, str(SYSTEM_TEST_ROOT))

BINARY_PATHS = {
    "cli": str(RUST_TARGET / "telepathy-cli"),
}
FAILED_PROFILES: dict[str, str] = {}
FAILED_ORDER_SEEDS: dict[str, str] = {}
SYSTEM_TEST_ORDER_SEED_ENV = "SYSTEM_TEST_ORDER_SEED"
SYSTEM_TEST_ORDER_SEED_PROPERTY = "system_test_order_seed"

def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--artifacts-dir",
        action="store",
        default=str(SYSTEM_TEST_ROOT / "artifacts"),
        help="Directory for system-test failure artifacts.",
    )
    parser.addoption(
        "--save-artifacts",
        action="store",
        choices=("failures", "all", "none"),
        default="failures",
        help="Save system-test artifacts for failures, all tests, or none.",
    )
    parser.addoption(
        "--test-iterations",
        action="store",
        type=int,
        default=8,
        help="Run each collected test this many times.",
    )


def _ensure_relay_certs() -> None:
    if all(path.exists() for path in RELAY_CERT_FILES):
        return

    result = subprocess.run(
        ["bash", str(GEN_CERTS_SCRIPT)],
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0 or not all(path.exists() for path in RELAY_CERT_FILES):
        message = (
            "relay TLS certificates are missing and could not be generated.\n"
            f"expected files:\n"
            + "\n".join(f"  - {path}" for path in RELAY_CERT_FILES)
            + f"\nstdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )
        raise pytest.UsageError(message)


def pytest_configure(config: pytest.Config) -> None:
    if not hasattr(config, "workerinput"):
        FAILED_PROFILES.clear()
        FAILED_ORDER_SEEDS.clear()

    _ensure_relay_certs()

    # result = subprocess.run(
    #     ["bash", str(BUILD_SCRIPT)],
    #     cwd=str(REPO_ROOT),
    #     capture_output=True,
    #     text=True,
    #     check=False,
    # )
    # if result.returncode != 0:
    #     message = (
    #         "system-test build failed.\n"
    #         f"stdout:\n{result.stdout}\n"
    #         f"stderr:\n{result.stderr}"
    #     )
    #     raise pytest.UsageError(message)

    config._system_test_binary_paths = BINARY_PATHS
    config._system_test_artifacts_dir = Path(config.getoption("artifacts_dir")).resolve()
    config._system_test_save_artifacts = str(config.getoption("save_artifacts"))
    config._system_test_order_seed = _resolve_order_seed(config)
    config._system_test_artifacts_dir.mkdir(parents=True, exist_ok=True)


def pytest_configure_node(node: pytest.Node) -> None:
    seed = getattr(node.config, "_system_test_order_seed", None)
    if isinstance(seed, str):
        node.workerinput[SYSTEM_TEST_ORDER_SEED_PROPERTY] = seed


def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]) -> None:
    seed = getattr(config, "_system_test_order_seed", None)
    _seeded_shuffle(items, seed)


def pytest_generate_tests(metafunc: pytest.Metafunc) -> None:
    if "iteration_id" not in metafunc.fixturenames:
        return

    iterations = int(metafunc.config.getoption("test_iterations") or 1)
    if iterations < 1:
        raise pytest.UsageError("--test-iterations must be >= 1")

    ids = [f"iter-{index}" for index in range(iterations)]
    metafunc.parametrize("iteration_id", [str(index) for index in range(iterations)], ids=ids)


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item: pytest.Item, call: pytest.CallInfo[Any]) -> Any:
    outcome = yield
    rep = outcome.get_result()
    setattr(item, f"rep_{rep.when}", rep)

    profile = getattr(item, "callspec", None)
    params = getattr(profile, "params", {})
    profile_name = getattr(params.get("profile"), "name", None)
    if isinstance(profile_name, str):
        rep.user_properties.append(("system_test_profile", profile_name))

    order_seed = getattr(item.config, "_system_test_order_seed", None)
    if isinstance(order_seed, str):
        rep.user_properties.append((SYSTEM_TEST_ORDER_SEED_PROPERTY, order_seed))


def pytest_runtest_logreport(report: pytest.TestReport) -> None:
    if not report.failed:
        return

    profile = next(
        (
            value
            for key, value in report.user_properties
            if key == "system_test_profile" and isinstance(value, str)
        ),
        "none",
    )
    FAILED_PROFILES.setdefault(report.nodeid, profile)

    order_seed = next(
        (
            value
            for key, value in report.user_properties
            if key == SYSTEM_TEST_ORDER_SEED_PROPERTY and isinstance(value, str)
        ),
        None,
    )
    if isinstance(order_seed, str):
        FAILED_ORDER_SEEDS.setdefault(report.nodeid, order_seed)


def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
    _ = exitstatus
    if hasattr(session.config, "workerinput"):
        return

    for nodeid, profile in sorted(FAILED_PROFILES.items()):
        seed = FAILED_ORDER_SEEDS.get(nodeid)
        if isinstance(seed, str):
            print(f"{nodeid} profile={profile} system_test_order_seed={seed}")
        else:
            print(f"{nodeid} profile={profile}")


def pytest_report_header(config: pytest.Config) -> list[str]:
    seed = getattr(config, "_system_test_order_seed", None)
    if isinstance(seed, str):
        return [f"system_test_order_seed={seed}"]
    return []


def _sanitize_nodeid(nodeid: str) -> str:
    safe = []
    for char in nodeid:
        if char.isalnum() or char in ("-", "_", "."):
            safe.append(char)
        else:
            safe.append("_")
    return "".join(safe)


def _serialize_topology(topology: TopologyManager) -> dict[str, Any]:
    discovery: dict[str, dict[str, str]] = {}
    for namespace in topology.client_namespaces:
        discovery[namespace] = {
            "relay_url": topology.relay_url(namespace),
            "dns_endpoint": topology.dns_endpoint(namespace),
            "dns_origin_domain": topology.dns_origin_domain(namespace),
            "pkarr_relay": topology.pkarr_relay(namespace),
        }
    return {
        "client_namespaces": list(topology.client_namespaces),
        "discovery": discovery,
    }


def _serialize_cli_pair(cli_pair: dict[str, CliProcess]) -> dict[str, Any]:
    return {
        actor_name: {
            "stdout": actor.stdout_lines(),
            "stderr": actor.stderr_lines(),
        }
        for actor_name, actor in cli_pair.items()
    }


def _serialize_cli_fixture(value: Any) -> dict[str, Any] | None:
    if isinstance(value, dict):
        return _serialize_cli_pair(value)
    actors = getattr(value, "actors", None)
    if isinstance(actors, dict):
        return _serialize_cli_pair(actors)
    return None


def _serialize_profile(profile: Any) -> Any:
    if hasattr(profile, "__dict__"):
        return dict(vars(profile))
    return repr(profile)


def _seeded_shuffle(items: list[Any], seed: str | None) -> None:
    if isinstance(seed, str) and len(items) > 1:
        Random(seed).shuffle(items)


def _build_debug_artifact_payload(
    *,
    nodeid: str,
    failed: bool,
    setup_report: Any,
    call_report: Any,
    teardown_report: Any,
    funcargs: dict[str, Any],
    order_seed: str | None,
) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "nodeid": nodeid,
        "failed": failed,
        "reports": {
            "setup": getattr(setup_report, "longreprtext", ""),
            "call": getattr(call_report, "longreprtext", ""),
            "teardown": getattr(teardown_report, "longreprtext", ""),
        },
    }

    profile = funcargs.get("profile")
    topology = funcargs.get("topology")

    if profile is not None:
        payload["profile"] = _serialize_profile(profile)
    if topology is not None:
        payload["topology"] = _serialize_topology(topology)
    for fixture_name in ("cli_pair", "room_cli_three", "room_cli_four", "room_cli_twenty"):
        serialized = _serialize_cli_fixture(funcargs.get(fixture_name))
        if serialized is not None:
            payload[fixture_name] = serialized
    if failed and isinstance(order_seed, str):
        payload[SYSTEM_TEST_ORDER_SEED_PROPERTY] = order_seed

    return payload


def _resolve_order_seed(config: pytest.Config) -> str | None:
    workerinput = getattr(config, "workerinput", None)
    if isinstance(workerinput, dict):
        seed = workerinput.get(SYSTEM_TEST_ORDER_SEED_PROPERTY)
        if isinstance(seed, str) and seed:
            return seed

    seed = os.environ.get(SYSTEM_TEST_ORDER_SEED_ENV)
    if isinstance(seed, str) and seed:
        return seed

    return None


@pytest.fixture(autouse=True)
def record_test_artifacts(request: pytest.FixtureRequest) -> Any:
    yield

    config = request.config
    save_mode = getattr(config, "_system_test_save_artifacts", "failures")
    if save_mode == "none":
        return

    setup_report = getattr(request.node, "rep_setup", None)
    call_report = getattr(request.node, "rep_call", None)
    teardown_report = getattr(request.node, "rep_teardown", None)

    failed = any(
        report is not None and report.failed
        for report in (setup_report, call_report, teardown_report)
    )
    if save_mode == "failures" and not failed:
        return

    artifacts_root = getattr(config, "_system_test_artifacts_dir", SYSTEM_TEST_ROOT / "artifacts")
    timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    test_dir = artifacts_root / f"{_sanitize_nodeid(request.node.nodeid)}__{timestamp}"
    test_dir.mkdir(parents=True, exist_ok=True)

    funcargs = getattr(request.node, "funcargs", {})
    payload = _build_debug_artifact_payload(
        nodeid=request.node.nodeid,
        failed=failed,
        setup_report=setup_report,
        call_report=call_report,
        teardown_report=teardown_report,
        funcargs=funcargs,
        order_seed=getattr(config, "_system_test_order_seed", None),
    )

    payload_path = test_dir / "debug.json"
    payload_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")


@pytest.fixture
def worker_tag() -> str:
    worker = os.environ.get("PYTEST_XDIST_WORKER", "0")
    if worker in {"", "master"}:
        return "0"

    match = re.search(r"(\d+)$", worker)
    if not match:
        return "0"
    return match.group(1)


@pytest.fixture
def iteration_id(request: pytest.FixtureRequest) -> str:
    param = getattr(request, "param", "0")
    return str(param)


@pytest.fixture(autouse=True)
def _attach_iteration_id(iteration_id: str) -> None:
    _ = iteration_id


@pytest.fixture(scope="session")
def binaries(pytestconfig: pytest.Config) -> dict[str, str]:
    return pytestconfig._system_test_binary_paths
