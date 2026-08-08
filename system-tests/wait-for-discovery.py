from __future__ import annotations

import sys
from pathlib import Path

SYSTEM_TEST_ROOT = Path(__file__).resolve().parent
if str(SYSTEM_TEST_ROOT) not in sys.path:
    sys.path.insert(0, str(SYSTEM_TEST_ROOT))

from harness.discovery import (
    DnsUdpReadinessProbe,
    HttpReadinessProbe,
    ReadinessTimeoutError,
    wait_for_readiness,
)


def main(argv: list[str]) -> int:
    host = argv[0] if argv else "127.0.0.1"
    try:
        wait_for_readiness(
            (
                HttpReadinessProbe(host, 3340, "/"),
                HttpReadinessProbe(host, 8080, "/pkarr"),
                DnsUdpReadinessProbe(host, 5300),
            ),
            timeout=30.0,
        )
    except ReadinessTimeoutError as error:
        print(f"discovery readiness failed: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
