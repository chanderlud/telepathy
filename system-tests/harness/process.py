from __future__ import annotations

import asyncio
import json
import signal
import uuid
from contextlib import suppress
from typing import Callable

_UNIT_COMMANDS = {
    "start_manager",
    "restart_manager",
    "shutdown",
    "end_call",
    "audio_test",
    "list_devices",
}
_SUBPROCESS_STREAM_LIMIT = 1024 * 1024


def _build_exec_args(
    namespace: str | None, binary: str, args: list[str]
) -> list[str]:
    if namespace:
        return ["ip", "netns", "exec", namespace, binary, *args]
    return [binary, *args]


class CliProcess:
    def __init__(
        self,
        binary_path: str,
        namespace: str,
        listen_port: int,
        bind_addresses: list[str],
        relay_url: str | None = None,
        dns_endpoint: str | None = None,
        pkarr_relay: str | None = None,
    ) -> None:
        self.binary_path = binary_path
        self.namespace = namespace
        self.listen_port = listen_port
        self.bind_addresses = bind_addresses
        self.relay_url = relay_url
        self.dns_endpoint = dns_endpoint
        self.pkarr_relay = pkarr_relay

        self._proc: asyncio.subprocess.Process | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._stdout_task: asyncio.Task[None] | None = None
        self.identity_peer_id: str | None = None

        self._stderr_log: list[str] = []
        self._stdout_log: list[dict] = []
        self._pending: dict[str, asyncio.Future[dict]] = {}
        self._events: asyncio.Queue[dict] = asyncio.Queue()

    async def start(self) -> None:
        args = ["--listen-port", str(self.listen_port)]
        for bind_address in self.bind_addresses:
            args.extend(["--bind-address", bind_address])
        if self.relay_url is not None:
            args.extend(["--relay-url", self.relay_url])
        if self.dns_endpoint is not None:
            args.extend(["--dns-endpoint", self.dns_endpoint])
        if self.pkarr_relay is not None:
            args.extend(["--pkarr-relay", self.pkarr_relay])
        exec_args = _build_exec_args(self.namespace, self.binary_path, args)
        self._proc = await asyncio.create_subprocess_exec(
            *exec_args,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            limit=_SUBPROCESS_STREAM_LIMIT,
        )
        self._stderr_task = asyncio.create_task(self._drain_stderr())
        self._stdout_task = asyncio.create_task(self._drain_stdout())

    async def send(self, command: dict) -> dict:
        if self._proc is None or self._proc.stdin is None:
            raise RuntimeError("CLI process is not started")

        request = _normalize_command_payload(command)
        request_id = request.get("id")
        if not request_id:
            request_id = str(uuid.uuid4())
            request["id"] = request_id

        if "cmd" not in request:
            raise ValueError("command payload requires 'cmd'")

        fut: asyncio.Future[dict] = asyncio.get_running_loop().create_future()
        self._pending[request_id] = fut

        payload = json.dumps(request, separators=(",", ":")) + "\n"
        self._proc.stdin.write(payload.encode())
        await self._proc.stdin.drain()
        return await fut

    async def expect_event(
        self, predicate: Callable[[dict], bool], timeout: float
    ) -> dict:
        async def _next_match() -> dict:
            while True:
                if self._proc is not None and self._proc.returncode is not None:
                    raise RuntimeError(
                        f"CLI process exited with code {self._proc.returncode}"
                    )
                event = await self._events.get()
                if predicate(event):
                    return event

        return await asyncio.wait_for(_next_match(), timeout=timeout)

    def stderr_lines(self) -> list[str]:
        return list(self._stderr_log)

    def stdout_lines(self) -> list[dict]:
        return list(self._stdout_log)

    async def _drain_stderr(self) -> None:
        assert self._proc is not None
        assert self._proc.stderr is not None
        while True:
            try:
                line = await self._proc.stderr.readline()
            except ValueError as error:
                self._stderr_log.append(f"stderr line exceeded stream limit: {error}")
                continue
            if not line:
                break
            self._stderr_log.append(line.decode(errors="replace").rstrip("\n"))

    async def _drain_stdout(self) -> None:
        assert self._proc is not None
        assert self._proc.stdout is not None
        while True:
            try:
                line = await self._proc.stdout.readline()
            except ValueError as error:
                self._stderr_log.append(f"stdout line exceeded stream limit: {error}")
                continue
            if not line:
                break

            text = line.decode(errors="replace").strip()
            if not text:
                continue

            try:
                msg = json.loads(text)
            except json.JSONDecodeError:
                self._stderr_log.append(f"invalid stdout json: {text}")
                continue
            self._stdout_log.append(msg)

            kind = msg.get("kind")
            if kind in {"ack", "result"}:
                request_id = msg.get("id")
                if request_id in self._pending and not self._pending[request_id].done():
                    self._pending[request_id].set_result(msg)
                    del self._pending[request_id]
            elif kind == "event":
                await self._events.put(msg)

    async def terminate(self) -> None:
        if self._proc is None:
            return

        if self._proc.stdin is not None and not self._proc.stdin.is_closing():
            self._proc.stdin.close()
            with suppress(Exception):
                await self._proc.stdin.wait_closed()

        if self._proc.returncode is None:
            self._proc.send_signal(signal.SIGTERM)
            try:
                await asyncio.wait_for(self._proc.wait(), timeout=5.0)
            except asyncio.TimeoutError:
                self._proc.kill()
                with suppress(Exception):
                    await self._proc.wait()

        for pending in self._pending.values():
            if not pending.done():
                pending.set_exception(RuntimeError("CLI process terminated"))
        self._pending.clear()

        if self._stdout_task is not None:
            self._stdout_task.cancel()
            with suppress(asyncio.CancelledError):
                await self._stdout_task
        if self._stderr_task is not None:
            self._stderr_task.cancel()
            with suppress(asyncio.CancelledError):
                await self._stderr_task

    async def restart(self) -> None:
        await self.terminate()

        self._stdout_log.clear()
        self._stderr_log.clear()
        self._pending.clear()
        while True:
            try:
                self._events.get_nowait()
            except asyncio.QueueEmpty:
                break
        self.identity_peer_id = None

        await self.start()


def _normalize_command_payload(command: dict) -> dict:
    request = dict(command)
    cmd = request.get("cmd")
    if not isinstance(cmd, str):
        return request

    args = request.get("args")
    normalized_args = dict(args) if isinstance(args, dict) else args

    if cmd == "add_contact" and isinstance(normalized_args, dict):
        if "contact_id" in normalized_args and "id" not in normalized_args:
            normalized_args["id"] = normalized_args.pop("contact_id")
        if "id" in normalized_args and "nickname" not in normalized_args:
            normalized_args["nickname"] = str(normalized_args["id"])

    if cmd in _UNIT_COMMANDS:
        if args is None or args == {}:
            request.pop("args", None)
        elif normalized_args is not args:
            request["args"] = normalized_args
        return request

    if args is None:
        request["args"] = {}
    elif normalized_args is not args:
        request["args"] = normalized_args

    return request
