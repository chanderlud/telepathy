from __future__ import annotations

import asyncio
import base64
import json
import os
import re
import signal
import uuid
from contextlib import suppress
from typing import Callable

_BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def peer_id_from_identity_key_b64(key_b64: str) -> str:
    key_bytes = base64.b64decode(key_b64)
    if len(key_bytes) != 68 or key_bytes[:4] != b"\x08\x01\x12\x40":
        raise AssertionError("unexpected protobuf-encoded ed25519 private key format")

    public_key = key_bytes[36:68]
    protobuf_public_key = b"\x08\x01\x12\x20" + public_key
    identity_multihash = b"\x00\x24" + protobuf_public_key
    return _base58btc_encode(identity_multihash)


def _base58btc_encode(raw: bytes) -> str:
    value = int.from_bytes(raw, "big")
    encoded = ""
    while value > 0:
        value, remainder = divmod(value, 58)
        encoded = _BASE58_ALPHABET[remainder] + encoded

    leading_zeroes = len(raw) - len(raw.lstrip(b"\x00"))
    return ("1" * leading_zeroes) + (encoded or "1")


_UNIT_COMMANDS = {
    "start_manager",
    "restart_manager",
    "shutdown",
    "end_call",
    "audio_test",
    "list_devices",
}


def _build_exec_args(
    namespace: str | None, binary: str, args: list[str]
) -> list[str]:
    if namespace:
        return ["ip", "netns", "exec", namespace, binary, *args]
    return [binary, *args]


class RelayProcess:
    def __init__(
        self,
        binary_path: str,
        namespace: str | None = None,
        listen_addr: str = "127.0.0.1:40142",
    ) -> None:
        self.binary_path = binary_path
        self.namespace = namespace
        self.listen_addr = listen_addr
        self.peer_id: str | None = None

        self._proc: asyncio.subprocess.Process | None = None
        self._stdout_task: asyncio.Task[None] | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._stdout_log: list[str] = []
        self._stderr_log: list[str] = []

    async def start(self) -> None:
        exec_args = _build_exec_args(self.namespace, self.binary_path, [])
        env = dict(os.environ)
        env.setdefault("RUST_LOG", "relay_server=info,libp2p=warn")
        self._proc = await asyncio.create_subprocess_exec(
            *exec_args,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=env,
        )
        self._stdout_task = asyncio.create_task(self._drain_stdout())
        self._stderr_task = asyncio.create_task(self._drain_stderr())
        try:
            await self._wait_for_peer_id(timeout=15.0)
        except Exception:
            await self.terminate()
            raise

    async def _wait_for_peer_id(self, timeout: float) -> None:
        assert self._proc is not None
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout

        while self.peer_id is None:
            self._extract_peer_id_from_logs()
            if self.peer_id is not None:
                return

            if self._proc.returncode is not None:
                # Catch a last log line that may have arrived after the
                # previous extraction pass but before this returncode check.
                self._extract_peer_id_from_logs()
                if self.peer_id is not None:
                    return
                stdout = "\n".join(self._stdout_log)
                stderr = "\n".join(self._stderr_log)
                raise RuntimeError(
                    f"relay process exited before peer id was detected "
                    f"(code={self._proc.returncode})\n"
                    f"stdout:\n{stdout}\n"
                    f"stderr:\n{stderr}"
                )

            remaining = deadline - loop.time()
            if remaining <= 0:
                # Avoid a deadline race where the peer id lands in logs right
                # as timeout is reached.
                self._extract_peer_id_from_logs()
                if self.peer_id is not None:
                    return
                stdout = "\n".join(self._stdout_log)
                stderr = "\n".join(self._stderr_log)
                raise TimeoutError(
                    "timed out waiting for relay peer id from relay logs.\n"
                    f"stdout:\n{stdout}\n"
                    f"stderr:\n{stderr}"
                )
            await asyncio.sleep(min(0.05, remaining))
        self._extract_peer_id_from_logs()

    def _extract_peer_id_from_logs(self) -> None:
        if self.peer_id is not None:
            return
        for line in (*self._stdout_log, *self._stderr_log):
            marker = "peer.id="
            marker_pos = line.find(marker)
            if marker_pos == -1:
                continue
            remainder = line[marker_pos + len(marker) :].strip()
            if not remainder:
                continue
            token = remainder.split()[0].strip().strip('"')
            if token:
                self.peer_id = token
                return

    async def _drain_stdout(self) -> None:
        assert self._proc is not None
        assert self._proc.stdout is not None
        while True:
            line = await self._proc.stdout.readline()
            if not line:
                break
            text = line.decode(errors="replace").rstrip("\n")
            self._stdout_log.append(text)
            self._maybe_capture_peer_id(text)

    async def _drain_stderr(self) -> None:
        assert self._proc is not None
        assert self._proc.stderr is not None

        while True:
            line = await self._proc.stderr.readline()
            if not line:
                break
            text = line.decode(errors="replace").rstrip("\n")
            self._stderr_log.append(text)
            self._maybe_capture_peer_id(text)

    def _maybe_capture_peer_id(self, line: str) -> None:
        if self.peer_id is not None:
            return
        marker = "peer.id="
        marker_pos = line.find(marker)
        if marker_pos == -1:
            return
        remainder = line[marker_pos + len(marker) :].strip()
        if not remainder:
            return
        token = remainder.split()[0].strip().strip('"')
        if token:
            self.peer_id = token

    def stderr_lines(self) -> list[str]:
        return list(self._stderr_log)

    def stdout_lines(self) -> list[str]:
        return list(self._stdout_log)

    async def terminate(self) -> None:
        if self._proc is None:
            return
        if self._proc.returncode is None:
            self._proc.send_signal(signal.SIGTERM)
            try:
                await asyncio.wait_for(self._proc.wait(), timeout=5.0)
            except asyncio.TimeoutError:
                self._proc.kill()
                with suppress(Exception):
                    await self._proc.wait()
        if self._stderr_task is not None:
            self._stderr_task.cancel()
            with suppress(asyncio.CancelledError):
                await self._stderr_task
        if self._stdout_task is not None:
            self._stdout_task.cancel()
            with suppress(asyncio.CancelledError):
                await self._stdout_task


class CliProcess:
    def __init__(
        self,
        binary_path: str,
        namespace: str,
        relay_addr: str,
        relay_peer: str,
    ) -> None:
        self.binary_path = binary_path
        self.namespace = namespace
        self.relay_addr = relay_addr
        self.relay_peer = relay_peer

        self._proc: asyncio.subprocess.Process | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._stdout_task: asyncio.Task[None] | None = None
        self.identity_peer_id: str | None = None

        self._stderr_log: list[str] = []
        self._stdout_log: list[dict] = []
        self._pending: dict[str, asyncio.Future[dict]] = {}
        self._events: asyncio.Queue[dict] = asyncio.Queue()

    async def start(self) -> None:
        args = [
            "--relay",
            self.relay_addr,
            "--relay-peer",
            self.relay_peer,
        ]
        exec_args = _build_exec_args(self.namespace, self.binary_path, args)
        self._proc = await asyncio.create_subprocess_exec(
            *exec_args,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
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
            line = await self._proc.stderr.readline()
            if not line:
                break
            self._stderr_log.append(line.decode(errors="replace").rstrip("\n"))

    async def _drain_stdout(self) -> None:
        assert self._proc is not None
        assert self._proc.stdout is not None
        while True:
            line = await self._proc.stdout.readline()
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

    async def _cleanup_after_exit(self, pending_error_message: str) -> None:
        for pending in self._pending.values():
            if not pending.done():
                pending.set_exception(RuntimeError(pending_error_message))
        self._pending.clear()

        if self._stdout_task is not None:
            self._stdout_task.cancel()
            with suppress(asyncio.CancelledError):
                await self._stdout_task
            self._stdout_task = None
        if self._stderr_task is not None:
            self._stderr_task.cancel()
            with suppress(asyncio.CancelledError):
                await self._stderr_task
            self._stderr_task = None

        self._proc = None

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

        await self._cleanup_after_exit("CLI process terminated")

    async def crash(self) -> None:
        if self._proc is None or self._proc.returncode is not None:
            return

        self._proc.kill()
        await self._proc.wait()
        await self._cleanup_after_exit("CLI process crashed")

    async def relaunch_same_identity(self, identity_key_b64: str) -> None:
        prior_peer_id = self.identity_peer_id
        if not isinstance(prior_peer_id, str):
            raise AssertionError("relaunch_same_identity requires a prior identity_peer_id")

        self._stdout_log.clear()
        self._stderr_log.clear()
        self._pending.clear()
        while True:
            try:
                self._events.get_nowait()
            except asyncio.QueueEmpty:
                break
        self.identity_peer_id = None

        # start() always assigns a new subprocess; _proc was cleared by crash()/terminate().
        await self.start()

        response = await self.send(
            {"cmd": "set_identity", "args": {"key_b64": identity_key_b64}}
        )
        if not response.get("ok"):
            raise AssertionError(f"set_identity after relaunch failed: {response}")

        derived = peer_id_from_identity_key_b64(identity_key_b64)
        if derived != prior_peer_id:
            raise AssertionError(
                f"identity key produced peer id {derived!r}, expected {prior_peer_id!r}"
            )
        self.identity_peer_id = prior_peer_id

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
