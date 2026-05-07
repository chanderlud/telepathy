from __future__ import annotations

import asyncio
import json
import re
from pathlib import Path
from typing import Any

import yaml

from .process import CliProcess


class ScenarioRunner:
    _VAR_PATTERN = re.compile(r"\$\{([^}]+)\}")
    _DEFAULT_SEND_TIMEOUT_SECONDS = 20.0

    def __init__(self) -> None:
        self._variables: dict[str, Any] = {}

    def load(self, scenario_path: Path) -> dict[str, Any]:
        with scenario_path.open("r", encoding="utf-8") as handle:
            return yaml.safe_load(handle)

    async def run(
        self,
        scenario: dict[str, Any],
        actors: dict[str, CliProcess],
        initial_variables: dict[str, Any] | None = None,
    ) -> None:
        if initial_variables:
            self._variables.update(initial_variables)

        steps = scenario.get("steps", [])
        for step in steps:
            actor_name = step.get("actor")
            if actor_name not in actors:
                raise AssertionError(f"unknown actor '{actor_name}' in scenario")
            actor = actors[actor_name]

            if "send" in step:
                command = self._resolve(step["send"])
                send_timeout = float(
                    step.get("send_timeout", self._DEFAULT_SEND_TIMEOUT_SECONDS)
                )
                try:
                    response = await asyncio.wait_for(
                        actor.send(command), timeout=send_timeout
                    )
                except asyncio.TimeoutError as exc:
                    diagnostics = self._format_diagnostics(actors)
                    raise AssertionError(
                        f"send command timed out for actor '{actor_name}' after "
                        f"{send_timeout:.1f}s.\n{diagnostics}"
                    ) from exc
                self._capture(actor_name, response)
                expected = step.get("expect_ack", {})
                try:
                    self._assert_subset(expected, response, f"{actor_name} send response")
                except Exception as exc:
                    diagnostics = self._format_diagnostics(actors)
                    raise AssertionError(
                        f"send expectation failed for actor '{actor_name}': {exc}\n{diagnostics}"
                    ) from exc

            if "expect_event" in step:
                event_spec = self._resolve(step["expect_event"])
                timeout = float(event_spec.get("timeout", 10.0))
                expected_type = event_spec.get("type")
                subset = event_spec.get("match", {})

                def _predicate(event: dict[str, Any]) -> bool:
                    if expected_type and event.get("type") != expected_type:
                        return False
                    return self._matches_subset(subset, event)

                try:
                    event = await actor.expect_event(_predicate, timeout=timeout)
                except Exception as exc:
                    diagnostics = self._format_diagnostics(actors)
                    raise AssertionError(
                        f"event expectation failed for actor '{actor_name}': {exc}\n{diagnostics}"
                    ) from exc

                self._capture(actor_name, event)
                self._assert_subset(subset, event, f"{actor_name} event")

    def _capture(self, actor_name: str, payload: dict[str, Any]) -> None:
        for key, value in payload.items():
            self._variables[f"{actor_name}.{key}"] = value

        data = payload.get("data")
        if isinstance(data, dict):
            for key, value in data.items():
                self._variables[f"{actor_name}.{key}"] = value

    def _resolve(self, value: Any) -> Any:
        if isinstance(value, dict):
            return {key: self._resolve(val) for key, val in value.items()}
        if isinstance(value, list):
            return [self._resolve(item) for item in value]
        if isinstance(value, str):
            return self._resolve_string(value)
        return value

    def _resolve_string(self, value: str) -> Any:
        full_match = self._VAR_PATTERN.fullmatch(value)
        if full_match:
            key = full_match.group(1)
            if key not in self._variables:
                raise AssertionError(f"missing scenario variable '{key}'")
            return self._variables[key]

        def _replace(match: re.Match[str]) -> str:
            key = match.group(1)
            if key not in self._variables:
                raise AssertionError(f"missing scenario variable '{key}'")
            return str(self._variables[key])

        return self._VAR_PATTERN.sub(_replace, value)

    def _assert_subset(
        self, expected: dict[str, Any], actual: dict[str, Any], label: str
    ) -> None:
        if not self._matches_subset(expected, actual):
            raise AssertionError(
                f"{label} mismatch\nexpected subset: {json.dumps(expected)}\n"
                f"actual payload: {json.dumps(actual)}"
            )

    def _matches_subset(self, expected: Any, actual: Any) -> bool:
        if isinstance(expected, dict):
            if not isinstance(actual, dict):
                return False
            for key, value in expected.items():
                if key not in actual:
                    return False
                if not self._matches_subset(value, actual[key]):
                    return False
            return True

        if expected == "*":
            return actual is not None

        if isinstance(expected, list):
            if not expected:
                return isinstance(actual, list) and len(actual) == 0
            if isinstance(actual, list):
                for item in expected:
                    if item not in actual:
                        return False
                return True
            if len(expected) == 1:
                return expected[0] == actual
            if isinstance(actual, str):
                return any(str(item) in actual for item in expected)
            return actual in expected

        return expected == actual

    def _format_diagnostics(self, actors: dict[str, CliProcess]) -> str:
        parts: list[str] = []
        for name, actor in actors.items():
            stdout = actor.stdout_lines()
            stderr = actor.stderr_lines()
            parts.append(
                f"== actor: {name} ==\n"
                f"stdout transcript: {json.dumps(stdout)}\n"
                f"stderr trace:\n" + "\n".join(stderr)
            )
        return "\n\n".join(parts)
