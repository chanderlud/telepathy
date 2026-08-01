from __future__ import annotations

from harness.scenario import ScenarioRunner


def test_matches_subset_accepts_tagged_enum_alternatives() -> None:
    runner = ScenarioRunner()

    assert runner._matches_subset(
        ["Connecting", "Connected"], {"Connected": {"peer_id": "peer-alpha"}}
    )
    assert runner._matches_subset(["Connecting", "Connected"], "Connecting")
    assert not runner._matches_subset(
        ["Connecting", "Connected"], {"Disconnected": {"reason": "closed"}}
    )
    assert runner._matches_subset(["alpha", "beta"], ["beta", "alpha", "gamma"])
