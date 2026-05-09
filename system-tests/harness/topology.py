from __future__ import annotations

import asyncio
import re
from dataclasses import dataclass


@dataclass(frozen=True)
class NetworkProfile:
    name: str
    delay_ms: int
    jitter_ms: int
    loss_pct: float
    burst_loss: bool
    seed: int


class TopologyManager:
    def __init__(self, worker_id: str = "0") -> None:
        self.worker_id = worker_id
        self._worker_index = self._parse_worker_index(worker_id)
        # relay-server currently binds a fixed port (40142)
        self.listen_port = 40142
        self.relay_namespace = f"ns-{worker_id}-relay"
        self.client_namespaces: list[str] = []
        self._created_namespaces: list[str] = []
        self._relay_ips: dict[str, str] = {}
        self._client_ifaces: dict[str, str] = {}

    @staticmethod
    def _parse_worker_index(worker_id: str) -> int:
        if worker_id == "master":
            return 0

        match = re.search(r"(\d+)$", worker_id)
        if not match:
            return 0
        return int(match.group(1))

    async def _run(self, *args: str) -> None:
        proc = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await proc.communicate()
        if proc.returncode != 0:
            raise RuntimeError(
                f"command failed: {' '.join(args)}\n"
                f"stdout: {stdout.decode(errors='replace')}\n"
                f"stderr: {stderr.decode(errors='replace')}"
            )

    async def setup(
        self,
        num_clients: int,
        profile: NetworkProfile,
        relay_namespace: str | None = None,
    ) -> None:
        self.client_namespaces = [
            f"ns-{self.worker_id}-cli-{index}" for index in range(num_clients)
        ]
        relay_namespace_owned = relay_namespace is None
        if relay_namespace is not None:
            self.relay_namespace = relay_namespace
            self.listen_port = 40142

        all_namespaces = [*self.client_namespaces]
        if relay_namespace_owned:
            all_namespaces.insert(0, self.relay_namespace)
        self._created_namespaces.clear()
        self._relay_ips.clear()
        self._client_ifaces.clear()

        try:
            for namespace in all_namespaces:
                await self._delete_namespace_if_exists(namespace)
                await self._run("ip", "netns", "add", namespace)
                self._created_namespaces.append(namespace)
                await self._run(
                    "ip",
                    "netns",
                    "exec",
                    namespace,
                    "ip",
                    "link",
                    "set",
                    "lo",
                    "up",
                )

            for index, client_ns in enumerate(self.client_namespaces):
                relay_iface = f"vr{self._worker_index}_{index}"
                client_iface = f"vc{self._worker_index}_{index}"
                subnet_octet = 10 + index
                relay_ip = f"10.0.{subnet_octet}.1"
                client_ip = f"10.0.{subnet_octet}.2"

                self._relay_ips[client_ns] = relay_ip
                self._client_ifaces[client_ns] = client_iface

                await self._delete_link_in_namespace_if_exists(
                    self.relay_namespace,
                    relay_iface,
                )
                await self._delete_link_if_exists(relay_iface)
                await self._delete_link_if_exists(client_iface)
                await self._run(
                    "ip",
                    "link",
                    "add",
                    relay_iface,
                    "type",
                    "veth",
                    "peer",
                    "name",
                    client_iface,
                )
                await self._run("ip", "link", "set", relay_iface, "netns", self.relay_namespace)
                await self._run("ip", "link", "set", client_iface, "netns", client_ns)

                await self._run(
                    "ip",
                    "netns",
                    "exec",
                    self.relay_namespace,
                    "ip",
                    "addr",
                    "add",
                    f"{relay_ip}/30",
                    "dev",
                    relay_iface,
                )
                await self._run(
                    "ip",
                    "netns",
                    "exec",
                    self.relay_namespace,
                    "ip",
                    "link",
                    "set",
                    relay_iface,
                    "up",
                )

                await self._run(
                    "ip",
                    "netns",
                    "exec",
                    client_ns,
                    "ip",
                    "addr",
                    "add",
                    f"{client_ip}/30",
                    "dev",
                    client_iface,
                )
                await self._run(
                    "ip",
                    "netns",
                    "exec",
                    client_ns,
                    "ip",
                    "link",
                    "set",
                    client_iface,
                    "up",
                )

                await self._run(
                    "ip",
                    "netns",
                    "exec",
                    client_ns,
                    "ip",
                    "route",
                    "replace",
                    "default",
                    "via",
                    relay_ip,
                )
                await self._apply_profile(client_ns, profile)
        except Exception:
            await self.teardown()
            raise

    async def _apply_profile(
        self,
        client_namespace: str,
        profile: NetworkProfile,
    ) -> None:
        if profile.delay_ms == 0 and profile.loss_pct == 0:
            return

        iface = self._client_ifaces[client_namespace]
        cmd = [
            "ip",
            "netns",
            "exec",
            client_namespace,
            "tc",
            "qdisc",
            "replace",
            "dev",
            iface,
            "root",
            "netem",
        ]
        if profile.delay_ms > 0:
            cmd.extend(["delay", f"{profile.delay_ms}ms"])
            if profile.jitter_ms > 0:
                cmd.append(f"{profile.jitter_ms}ms")
        if profile.loss_pct > 0:
            cmd.extend(["loss", "random", f"{profile.loss_pct}%"])
            if profile.burst_loss:
                cmd.append("25%")
        await self._run(*cmd)

    async def _delete_namespace_if_exists(self, namespace: str) -> None:
        proc = await asyncio.create_subprocess_exec(
            "ip",
            "netns",
            "del",
            namespace,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        await proc.wait()

    async def _delete_link_if_exists(self, iface: str) -> None:
        proc = await asyncio.create_subprocess_exec(
            "ip",
            "link",
            "del",
            iface,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        await proc.wait()

    async def _delete_link_in_namespace_if_exists(self, namespace: str, iface: str) -> None:
        proc = await asyncio.create_subprocess_exec(
            "ip",
            "netns",
            "exec",
            namespace,
            "ip",
            "link",
            "del",
            iface,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        await proc.wait()

    async def teardown(self) -> None:
        for namespace in reversed(self._created_namespaces):
            proc = await asyncio.create_subprocess_exec(
                "ip",
                "netns",
                "del",
                namespace,
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.DEVNULL,
            )
            await proc.wait()

        self._created_namespaces.clear()
        self._relay_ips.clear()
        self._client_ifaces.clear()
        self.client_namespaces = []

    def relay_addr(self, namespace: str) -> str:
        relay_ip = self._relay_ips.get(namespace, "127.0.0.1")
        return f"{relay_ip}:{self.listen_port}"
