from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from typing import Any

import pytest

from harness.process import CliProcess, RelayProcess
from harness.topology import TopologyManager


SYSTEM_TEST_ROOT = Path(__file__).resolve().parent
REPO_ROOT = SYSTEM_TEST_ROOT.parent
BUILD_SCRIPT = SYSTEM_TEST_ROOT / "build.sh"
RUST_TARGET = REPO_ROOT / "rust" / "target" / "debug"

if str(SYSTEM_TEST_ROOT) not in sys.path:
    sys.path.insert(0, str(SYSTEM_TEST_ROOT))

BINARY_PATHS = {
    "relay": str(RUST_TARGET / "relay-server"),
    "cli": str(RUST_TARGET / "telepathy-cli"),
}

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
        default=4,
        help="Run each collected test this many times.",
    )


def pytest_configure(config: pytest.Config) -> None:
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
    config._system_test_artifacts_dir.mkdir(parents=True, exist_ok=True)


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


def _sanitize_nodeid(nodeid: str) -> str:
    safe = []
    for char in nodeid:
        if char.isalnum() or char in ("-", "_", "."):
            safe.append(char)
        else:
            safe.append("_")
    return "".join(safe)


def _serialize_topology(topology: TopologyManager) -> dict[str, Any]:
    return {
        "relay_namespace": topology.relay_namespace,
        "client_namespaces": list(topology.client_namespaces),
    }


def _serialize_relay(relay: RelayProcess) -> dict[str, Any]:
    return {
        "peer_id": relay.peer_id,
        "stdout": relay.stdout_lines(),
        "stderr": relay.stderr_lines(),
    }


def _serialize_cli_pair(cli_pair: dict[str, CliProcess]) -> dict[str, Any]:
    return {
        actor_name: {
            "stdout": actor.stdout_lines(),
            "stderr": actor.stderr_lines(),
        }
        for actor_name, actor in cli_pair.items()
    }


def _serialize_profile(profile: Any) -> Any:
    if hasattr(profile, "__dict__"):
        return dict(vars(profile))
    return repr(profile)


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
    timestamp = datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
    test_dir = artifacts_root / f"{_sanitize_nodeid(request.node.nodeid)}__{timestamp}"
    test_dir.mkdir(parents=True, exist_ok=True)

    payload: dict[str, Any] = {
        "nodeid": request.node.nodeid,
        "failed": failed,
        "reports": {
            "setup": getattr(setup_report, "longreprtext", ""),
            "call": getattr(call_report, "longreprtext", ""),
            "teardown": getattr(teardown_report, "longreprtext", ""),
        },
    }

    funcargs = getattr(request.node, "funcargs", {})
    profile = funcargs.get("profile")
    topology = funcargs.get("topology")
    relay = funcargs.get("relay")
    cli_pair = funcargs.get("cli_pair")

    if profile is not None:
        payload["profile"] = _serialize_profile(profile)
    if topology is not None:
        payload["topology"] = _serialize_topology(topology)
    if relay is not None:
        payload["relay"] = _serialize_relay(relay)
    if cli_pair is not None:
        payload["cli_pair"] = _serialize_cli_pair(cli_pair)

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
