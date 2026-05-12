from __future__ import annotations

import asyncio
import contextlib
import json
import re
from pathlib import Path
from typing import Any

import yaml

from .process import CliProcess
from .topology import TopologyManager


class ScenarioRunner:
    _VAR_PATTERN = re.compile(r"\$\{([^}]+)\}")
    _DEFAULT_SEND_TIMEOUT_SECONDS = 20.0

    def __init__(
        self,
        topology: TopologyManager | None = None,
        actor_namespaces: dict[str, str] | None = None,
    ) -> None:
        self._topology = topology
        self._actor_namespaces = dict(actor_namespaces or {})
        self._background_tasks: list[asyncio.Task[None]] = []
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
        self._background_tasks.clear()
        if initial_variables:
            self._variables.update(initial_variables)

        steps = scenario.get("steps", [])
        run_completed_normally = False
        try:
            for step in steps:
                if "concurrent" in step:
                    await self._run_concurrent_step(step["concurrent"], actors)
                    continue

                if "restart_actor" in step:
                    await self._run_restart_actor_step(step["restart_actor"], actors)
                    continue

                if "crash_actor" in step:
                    await self._run_crash_actor_step(step["crash_actor"], actors)
                    continue

                if "relaunch_actor" in step:
                    await self._run_relaunch_actor_step(step["relaunch_actor"], actors)
                    continue

                if "block_namespace" in step:
                    await self._run_block_namespace_step(step["block_namespace"], actors)
                    continue

                if "restart_link" in step:
                    await self._run_restart_link_step(step["restart_link"], actors)
                    continue

                if "flap_link" in step:
                    await self._run_flap_link_step(step["flap_link"], actors)
                    continue

                if "sleep" in step:
                    await asyncio.sleep(float(self._resolve(step["sleep"])))
                    continue

                actor_name = step.get("actor")
                if actor_name not in actors:
                    raise AssertionError(f"unknown actor '{actor_name}' in scenario")
                actor = actors[actor_name]

                if "send" in step:
                    await self._run_send_step(
                        actor_name=actor_name,
                        actor=actor,
                        step=step,
                        actors=actors,
                    )

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
            run_completed_normally = True
        finally:
            tasks = list(self._background_tasks)
            self._background_tasks.clear()
            if tasks:
                if run_completed_normally:
                    await asyncio.gather(*tasks)
                else:
                    for task in tasks:
                        if not task.done():
                            task.cancel()
                    for task in tasks:
                        with contextlib.suppress(asyncio.CancelledError, Exception):
                            await task

    def _require_topology(self) -> TopologyManager:
        if self._topology is None:
            raise AssertionError(
                "this scenario step requires a TopologyManager; pass topology= to ScenarioRunner"
            )
        return self._topology

    def _namespace_for_actor(self, actor_name: str) -> str:
        ns = self._actor_namespaces.get(actor_name)
        if not ns:
            raise AssertionError(
                "this scenario step requires actor_namespaces mapping; pass actor_namespaces= "
                "to ScenarioRunner"
            )
        return ns

    async def _run_crash_actor_step(
        self, actor_name: str, actors: dict[str, CliProcess]
    ) -> None:
        name = str(self._resolve(actor_name))
        if name not in actors:
            raise AssertionError(f"unknown actor '{name}' in scenario")
        await actors[name].crash()

    async def _run_relaunch_actor_step(
        self, spec: dict[str, Any], actors: dict[str, CliProcess]
    ) -> None:
        resolved = self._resolve(spec)
        if not isinstance(resolved, dict):
            raise AssertionError("relaunch_actor step must be a mapping")
        actor_name = resolved.get("actor")
        if not isinstance(actor_name, str) or actor_name not in actors:
            raise AssertionError("relaunch_actor requires string 'actor' naming a known actor")

        identity_key = resolved.get("identity_key_b64")
        if identity_key is None:
            identity_key = self._variables.get(f"{actor_name}.identity_key_b64")
        if not isinstance(identity_key, str):
            raise AssertionError(
                f"missing identity key for relaunch_actor (provide identity_key_b64 or "
                f"variable {actor_name}.identity_key_b64)"
            )

        await actors[actor_name].relaunch_same_identity(identity_key)

    async def _run_block_namespace_step(
        self, spec: dict[str, Any], actors: dict[str, CliProcess]
    ) -> None:
        topology = self._require_topology()
        resolved = self._resolve(spec)
        if not isinstance(resolved, dict):
            raise AssertionError("block_namespace step must be a mapping")
        actor_name = resolved.get("actor")
        if not isinstance(actor_name, str) or actor_name not in actors:
            raise AssertionError("block_namespace requires string 'actor' naming a known actor")

        duration = float(resolved["duration"])
        wait = bool(resolved.get("wait", False))
        namespace = self._namespace_for_actor(actor_name)

        if wait:
            await topology.block_namespace(namespace, duration)
        else:
            self._background_tasks.append(
                topology.block_namespace_for(namespace, duration)
            )

    async def _run_restart_link_step(
        self, spec: dict[str, Any], actors: dict[str, CliProcess]
    ) -> None:
        topology = self._require_topology()
        resolved = self._resolve(spec)
        if not isinstance(resolved, dict):
            raise AssertionError("restart_link step must be a mapping")
        actor_name = resolved.get("actor")
        if not isinstance(actor_name, str) or actor_name not in actors:
            raise AssertionError("restart_link requires string 'actor' naming a known actor")

        down_seconds = float(resolved.get("down_seconds", 0.5))
        namespace = self._namespace_for_actor(actor_name)
        await topology.restart_link(namespace, down_seconds)

    async def _run_flap_link_step(
        self, spec: dict[str, Any], actors: dict[str, CliProcess]
    ) -> None:
        topology = self._require_topology()
        resolved = self._resolve(spec)
        if not isinstance(resolved, dict):
            raise AssertionError("flap_link step must be a mapping")
        actor_name = resolved.get("actor")
        if not isinstance(actor_name, str) or actor_name not in actors:
            raise AssertionError("flap_link requires string 'actor' naming a known actor")

        count = int(resolved.get("count", 3))
        down_seconds = float(resolved.get("down_seconds", 0.5))
        up_seconds = float(resolved.get("up_seconds", 0.5))
        namespace = self._namespace_for_actor(actor_name)
        await topology.flap_link(
            namespace,
            count=count,
            down_seconds=down_seconds,
            up_seconds=up_seconds,
        )

    async def _run_send_step(
        self,
        actor_name: str,
        actor: CliProcess,
        step: dict[str, Any],
        actors: dict[str, CliProcess],
    ) -> dict[str, Any]:
        command = self._resolve(step["send"])
        send_timeout = float(step.get("send_timeout", self._DEFAULT_SEND_TIMEOUT_SECONDS))
        try:
            response = await asyncio.wait_for(actor.send(command), timeout=send_timeout)
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
        return response

    async def _run_concurrent_step(
        self, concurrent_steps: list[dict[str, Any]], actors: dict[str, CliProcess]
    ) -> None:
        prepared_steps: list[tuple[str, CliProcess, dict[str, Any], float]] = []
        for substep in concurrent_steps:
            actor_name = substep.get("actor")
            if actor_name not in actors:
                raise AssertionError(f"unknown actor '{actor_name}' in scenario")
            actor = actors[actor_name]
            command = self._resolve(substep.get("send"))
            send_timeout = float(
                substep.get("send_timeout", self._DEFAULT_SEND_TIMEOUT_SECONDS)
            )
            prepared_steps.append((actor_name, actor, command, send_timeout))

        send_tasks = [
            asyncio.wait_for(actor.send(command), timeout=send_timeout)
            for _, actor, command, send_timeout in prepared_steps
        ]

        try:
            responses = await asyncio.gather(*send_tasks)
        except asyncio.TimeoutError as exc:
            diagnostics = self._format_diagnostics(actors)
            raise AssertionError(
                f"concurrent send command timed out.\n{diagnostics}"
            ) from exc
        except Exception as exc:
            diagnostics = self._format_diagnostics(actors)
            raise AssertionError(
                f"concurrent send command failed: {exc}\n{diagnostics}"
            ) from exc

        for (actor_name, _, _, _), response, substep in zip(
            prepared_steps, responses, concurrent_steps
        ):
            self._capture(actor_name, response)
            expected = self._resolve(substep.get("expect_ack", {}))
            try:
                self._assert_subset(
                    expected, response, f"{actor_name} concurrent send response"
                )
            except Exception as exc:
                diagnostics = self._format_diagnostics(actors)
                raise AssertionError(
                    f"concurrent send expectation failed for actor '{actor_name}': "
                    f"{exc}\n{diagnostics}"
                ) from exc

    async def _run_restart_actor_step(
        self, actor_name: str, actors: dict[str, CliProcess]
    ) -> None:
        if actor_name not in actors:
            raise AssertionError(f"unknown actor '{actor_name}' in scenario")

        actor = actors[actor_name]
        prior_peer_id = actor.identity_peer_id
        await actor.restart()

        identity_key = self._variables.get(f"{actor_name}.identity_key_b64")
        if not isinstance(identity_key, str):
            raise AssertionError(
                f"missing scenario variable '{actor_name}.identity_key_b64' "
                "for restart_actor"
            )

        response = await actor.send(
            {"cmd": "set_identity", "args": {"key_b64": identity_key}}
        )
        self._assert_subset({"ok": True}, response, f"{actor_name} restart set_identity")
        actor.identity_peer_id = prior_peer_id
        if isinstance(prior_peer_id, str):
            self._variables[f"{actor_name}.peer_id"] = prior_peer_id

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
                return self._matches_subset(expected[0], actual)
            if all(isinstance(item, str) for item in expected) and isinstance(
                actual, str
            ):
                return actual in expected
            if all(isinstance(item, str) for item in expected) and isinstance(
                actual, dict
            ):
                if len(actual) != 1:
                    return False
                only_key = next(iter(actual.keys()))
                return isinstance(only_key, str) and only_key in expected
            if isinstance(actual, str):
                return any(str(item) in actual for item in expected)
            return actual in expected

        if isinstance(expected, str) and isinstance(actual, dict):
            return len(actual) == 1 and expected in actual

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
