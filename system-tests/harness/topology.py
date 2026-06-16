from __future__ import annotations

import asyncio
import ipaddress
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
    _CANONICAL_RELAY_IP = "100.64.0.1"
    _RELAY_PORT = 3340
    _DNS_ORIGIN_DOMAIN = "dns.iroh.test."

    def __init__(self, worker_id: str = "0") -> None:
        self.worker_id = worker_id
        self._worker_index = self._parse_worker_index(worker_id)
        self.client_namespaces: list[str] = []
        self._created_namespaces: list[str] = []
        self._gateway_ips: dict[str, str] = {}
        self._client_ifaces: dict[str, str] = {}
        self._root_ifaces: list[str] = []
        # Whether iptables is usable on this host. Disabled lazily if the
        # binary is missing so forwarding setup is a no-op where the FORWARD
        # policy already accepts inter-namespace traffic.
        self._iptables_available = True
        self._canonical_relay_url = (
            f"http://{self._CANONICAL_RELAY_IP}:{self._RELAY_PORT}"
        )

    @staticmethod
    def _parse_worker_index(worker_id: str) -> int:
        if worker_id == "master":
            return 0

        numbers = [int(match.group(0)) for match in re.finditer(r"\d+", worker_id)]
        if not numbers:
            return 0
        if len(numbers) == 1:
            return numbers[0]
        return numbers[-1] * 256 + numbers[0]

    @staticmethod
    def _interface_names(worker_index: int, index: int) -> tuple[str, str]:
        return f"vr{worker_index}_{index}", f"vc{worker_index}_{index}"

    @staticmethod
    def _client_addresses(
        worker_index: int,
        num_clients: int,
        index: int,
    ) -> tuple[str, str]:
        subnet_index = worker_index * max(1, num_clients) + index
        test_network = ipaddress.ip_network("10.0.0.0/8")
        subnet_size = 4
        max_subnets = test_network.num_addresses // subnet_size
        if subnet_index >= max_subnets:
            raise ValueError(
                f"topology subnet index {subnet_index} exceeds {test_network} capacity"
            )

        subnet_address = int(test_network.network_address) + subnet_index * subnet_size
        subnet = ipaddress.ip_network((subnet_address, 30))
        gateway_ip, client_ip = subnet.hosts()
        return str(gateway_ip), str(client_ip)

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

    async def _iptables(self, *args: str, check: bool = True) -> int:
        """Runs an iptables command, returning its exit code.

        If iptables is not installed the host is assumed to permit forwarding
        already (no Docker-imposed DROP policy), so forwarding management is
        disabled for the remainder of this manager's lifetime.
        """
        if not self._iptables_available:
            return 1
        try:
            proc = await asyncio.create_subprocess_exec(
                "iptables",
                *args,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except FileNotFoundError:
            self._iptables_available = False
            return 1
        stdout, stderr = await proc.communicate()
        returncode = proc.returncode
        if returncode is None:
            raise RuntimeError(f"command did not exit: iptables {' '.join(args)}")
        if check and returncode != 0:
            raise RuntimeError(
                f"command failed: iptables {' '.join(args)}\n"
                f"stdout: {stdout.decode(errors='replace')}\n"
                f"stderr: {stderr.decode(errors='replace')}"
            )
        return returncode

    async def _allow_forwarding(self, iface: str) -> None:
        """Inserts FORWARD ACCEPT rules so traffic to/from `iface` is routed.

        Rules are inserted at the head of the FORWARD chain so they take
        precedence over Docker's default DROP policy. Any stale duplicates from
        a previous run are removed first to keep the chain from growing without
        bound across repeated test sessions.
        """
        for direction in ("-i", "-o"):
            await self._remove_forward_rule(iface, direction)
            await self._iptables(
                "-I", "FORWARD", "1", direction, iface, "-j", "ACCEPT"
            )

    async def _remove_forward_rule(self, iface: str, direction: str) -> None:
        # A given rule may have been inserted multiple times historically;
        # delete until none remain.
        while (
            await self._iptables(
                "-D", "FORWARD", direction, iface, "-j", "ACCEPT", check=False
            )
            == 0
        ):
            pass

    async def _clear_forwarding(self, iface: str) -> None:
        for direction in ("-i", "-o"):
            await self._remove_forward_rule(iface, direction)

    async def setup(
        self,
        num_clients: int,
        profile: NetworkProfile,
    ) -> None:
        self.client_namespaces = [
            f"ns-{self.worker_id}-cli-{index}" for index in range(num_clients)
        ]
        self._created_namespaces.clear()
        self._gateway_ips.clear()
        self._client_ifaces.clear()
        self._root_ifaces.clear()

        try:
            # Expose a stable host-side relay identity that every namespace can route to.
            await self._run(
                "ip",
                "addr",
                "replace",
                f"{self._CANONICAL_RELAY_IP}/32",
                "dev",
                "lo",
            )

            for namespace in self.client_namespaces:
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
                gateway_iface, client_iface = self._interface_names(
                    self._worker_index,
                    index,
                )
                gateway_ip, client_ip = self._client_addresses(
                    self._worker_index,
                    num_clients,
                    index,
                )

                self._gateway_ips[client_ns] = gateway_ip
                self._client_ifaces[client_ns] = client_iface
                self._root_ifaces.append(gateway_iface)

                await self._delete_link_if_exists(gateway_iface)
                await self._delete_link_if_exists(client_iface)
                await self._run(
                    "ip",
                    "link",
                    "add",
                    gateway_iface,
                    "type",
                    "veth",
                    "peer",
                    "name",
                    client_iface,
                )
                await self._run("ip", "link", "set", client_iface, "netns", client_ns)

                await self._run(
                    "ip",
                    "addr",
                    "add",
                    f"{gateway_ip}/30",
                    "dev",
                    gateway_iface,
                )
                await self._run("ip", "link", "set", gateway_iface, "up")

                # Docker (used for the relay/DNS containers) sets the kernel
                # FORWARD policy to DROP, which silently blocks the direct
                # client-to-client path that iroh needs for holepunching.
                # Relay traffic is delivered locally and is unaffected, so the
                # symptom is "relay works, direct never forms" on every worker
                # whose gateway interfaces lack an explicit ACCEPT rule.
                await self._allow_forwarding(gateway_iface)

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
                    gateway_ip,
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

    async def teardown(self) -> None:
        for iface in self._root_ifaces:
            await self._clear_forwarding(iface)
            await self._delete_link_if_exists(iface)

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
        self._gateway_ips.clear()
        self._client_ifaces.clear()
        self._root_ifaces.clear()
        self.client_namespaces = []

    def relay_url(self, namespace: str) -> str:
        return self._canonical_relay_url

    def dns_endpoint(self, namespace: str) -> str:
        gateway_ip = self._gateway_ips.get(namespace, "127.0.0.1")
        return f"{gateway_ip}:5300"

    def dns_origin_domain(self, namespace: str) -> str:
        return self._DNS_ORIGIN_DOMAIN

    def pkarr_relay(self, namespace: str) -> str:
        gateway_ip = self._gateway_ips.get(namespace, "127.0.0.1")
        return f"http://{gateway_ip}:8080/pkarr"
