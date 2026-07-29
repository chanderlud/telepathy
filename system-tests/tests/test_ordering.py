from __future__ import annotations

from types import SimpleNamespace

import conftest as system_conftest


def test_seeded_shuffle_is_deterministic() -> None:
    items = [f"item-{index}" for index in range(20)]
    first = items.copy()
    second = items.copy()

    system_conftest._seeded_shuffle(first, "seed-alpha")
    system_conftest._seeded_shuffle(second, "seed-alpha")

    assert first == second


def test_seeded_shuffle_changes_with_seed() -> None:
    items_alpha = [f"item-{index}" for index in range(20)]
    items_beta = [f"item-{index}" for index in range(20)]

    system_conftest._seeded_shuffle(items_alpha, "seed-alpha")
    system_conftest._seeded_shuffle(items_beta, "seed-beta")

    assert items_alpha != items_beta


def test_report_header_shows_seed() -> None:
    config = SimpleNamespace(_system_test_order_seed="seed-alpha")

    assert system_conftest.pytest_report_header(config) == [
        "system_test_order_seed=seed-alpha"
    ]


def test_debug_artifact_payload_includes_order_seed() -> None:
    report = SimpleNamespace(longreprtext="boom")

    payload = system_conftest._build_debug_artifact_payload(
        nodeid="test_node",
        failed=True,
        setup_report=report,
        call_report=report,
        teardown_report=report,
        funcargs={},
        order_seed="seed-alpha",
    )

    assert payload["system_test_order_seed"] == "seed-alpha"
