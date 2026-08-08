from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from harness.process import CliProcess
from harness.scenario import ScenarioRunner


class _FakeActor(CliProcess):
    def __init__(self, messages: list[dict]) -> None:
        self.messages = messages

    def stdout_lines(self) -> list[dict]:
        return list(self.messages)

    def stderr_lines(self) -> list[str]:
        return []

    async def send(self, command: dict) -> dict:
        _ = command
        self.messages.append(
            {"kind": "event", "type": "forbidden", "source": "send"}
        )
        return {"kind": "ack", "ok": True}


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


@pytest.mark.asyncio
async def test_expect_absent_excludes_prebaseline_events_but_catches_send_event() -> None:
    actor = _FakeActor(
        [{"kind": "event", "type": "forbidden", "source": "old"}]
    )

    with pytest.raises(AssertionError, match='"source": "send"'):
        await ScenarioRunner().run(
            {
                "steps": [
                    {
                        "actor": "alice",
                        "send": {"cmd": "trigger"},
                        "expect_absent": {"type": "forbidden", "timeout": 0},
                    }
                ]
            },
            {"alice": actor},
        )


@pytest.mark.asyncio
async def test_expect_absent_fails_for_event_emitted_during_step_observation() -> None:
    actor = _FakeActor([])

    async def append_event_during_window(_window: float) -> None:
        await original_sleep(0)
        actor.messages.append(
            {"kind": "event", "type": "forbidden", "source": "window"}
        )

    original_sleep = asyncio.sleep
    asyncio.sleep = append_event_during_window
    try:
        with pytest.raises(AssertionError, match="unexpected event"):
            await ScenarioRunner().run(
                {
                    "steps": [
                        {
                            "actor": "alice",
                            "expect_absent": {"type": "forbidden", "timeout": 0},
                        }
                    ]
                },
                {"alice": actor},
            )
    finally:
        asyncio.sleep = original_sleep


def test_simultaneous_call_absence_starts_before_accept_call() -> None:
    scenario_path = (
        Path(__file__).parents[1]
        / "scenarios"
        / "session_simultaneous_dial_then_call.yaml"
    )
    scenario = ScenarioRunner().load(scenario_path)
    steps = scenario["steps"]
    accept_steps = [
        step
        for step in steps
        if step.get("send", {}).get("cmd") == "accept_call"
    ]

    assert len(accept_steps) == 1
    assert accept_steps[0]["expect_absent"] == {
        "type": "accept_call_prompt",
        "timeout": 5.0,
    }
    accept_index = steps.index(accept_steps[0])
    assert not any("expect_absent" in step for step in steps[accept_index + 1 :])
