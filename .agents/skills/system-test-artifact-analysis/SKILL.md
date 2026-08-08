---
name: system-test-artifact-analysis
description: Analyze Telepathy system-test failure artifacts from CI — debug.json structure, actor event timelines, tracing event vocabulary, and the pitfalls that mislead (monitor noise vs real churn, truncated-looking transcripts, relayed death-detection latency). Use when investigating failed system_tests CI jobs or local system-test artifacts.
---

# System Test Artifact Analysis

Analyze `debug.json` failure artifacts produced by `system-tests/` (CI job
`system_tests / System Tests [sweep N, seed <run>-<N>]`). Goal: reconstruct the
exact event sequence per actor, classify the failure (product bug vs
environmental flake vs scenario bug), and produce a fix or a deterministic
integration recreation.

## When to Use

- A `system_tests` CI job (or `System Tests Nightly`) went red.
- You have, or can download, `system-tests-artifacts-*` from a run.

## Getting the Artifacts

```sh
gh run list --branch <branch> --limit 5            # find the run
gh run view <run-id> --json jobs --jq '.jobs[] | select(.conclusion=="failure") | {name, databaseId}'
gh run download <run-id> --dir /tmp/artifacts -p "system-tests-artifacts-*"
```

One directory per failed attempt: `tests_test_scenarios.py__<test>_<iter>-<profile>___<timestamp>/debug.json`.
Sweeps differ only by seed (`<run-id>-<sweep-index>`); the same test failing on
several sweeps at once means near-deterministic under CI timing. A failure
repeating on the same sweep across runs but not others means seed-dependent
ordering.

## debug.json Structure

| Key | Contents |
|---|---|
| `nodeid` | full pytest id, including `[iter-N-<profile>]` |
| `reports.setup` / `reports.call` / `reports.teardown` | pytest longrepr text; the failing step and (for YAML-scenario failures) embedded per-actor diagnostics |
| `profile` | network profile params (delay_ms, jitter_ms, loss_pct, burst_loss) |
| `topology` | namespaces, relay/dns endpoints |
| `cli_pair` / `room_cli_three` / ... | **per-actor full `stdout` (every JSON event/ack) and `stderr` (every tracing log line)** — the primary evidence |
| `system_test_order_seed` | ordering seed for the sweep |

**Read `cli_pair[actor].stderr` first, not the exception text.** The exception
in `reports.call` embeds diagnostics, but for Python-written tests (not YAML
scenarios) it is a bare traceback with no transcripts. `cli_pair` always has
the complete logs.

## Event Vocabulary (stderr, `event="..."`)

Session/collision: `session_collision_kept_new`, `session_collision_kept_existing`, `session_collision_deferred_candidate`, `session_collision_candidate_promoted`, `session_stopped`, `session_cleaned_up`, `session_error_critical` (with `error=Error { kind: TransportRecv }` etc.), `session_message_received` (`Ok(Hello…)`, `Ok(HelloAck…)`, `Ok(KeepAlive)`), `session_message_unexpected`, `session_rearmed_pending_outgoing`.

Negotiation: `outgoing_negotiation_waiting_hello_ack`, `hello_ack_timeout`, `session_candidate_resolution_wait`, `accept_prompt_offer_expired`, `accept_prompt_transferred`, `room_goodbye_during_negotiation_grace`, `simultaneous_dial_detected_winning|_yielding`, `room_join_received`, `room_duplicate_join_replacing_connection`, `room_cancelled_sending_goodbye`, `room_handshake_unexpected_message`.

Manager/transport: `manager_endpoint_setup`, `dial_initial`, `ignored_redundant_outgoing`, `incoming_connection`, `incoming_connection_failed`, `accept_incoming_failed` (`authentication failed` = the peer aborted mid-handshake, usually a redundant dial being closed — often benign), `connect_succeeded`, `connect_failed`.

stdout event types: `ready`, `manager_active` (`{"Starting":null}`/`{"Active":null}`), `session_status` (`Connecting`/`Connected`/`Inactive`), `accept_call_prompt` / `accept_call_canceled` (with `request_id`), `call_state` (`Waiting`/`Connected`/`CallEnded`/`RoomJoin`/`RoomLeave`), `statistics`.

## Flows That Worked

1. **Per-actor timelines.** Extract and sort events per actor, filtering noise:

```python
import json, re, glob
p = glob.glob('/tmp/artifacts/*/*/debug.json')[0]
doc = json.load(open(p))
for actor, data in doc['cli_pair'].items():
    print('---', actor)
    for line in data['stderr']:
        line = re.sub(r'\x1b\[[0-9;]*m', '', line)
        m = re.search(r'(\d{2}:\d{2}:\d{2}\.\d+).*event="([a-z_]+)"', line)
        if m and m.group(2) not in ('session_waiting_for_event','connection_path','session_keep_alive_sent','session_continuing_after_call'):
            print(' ', m.group(1), m.group(2))
```

2. **Correlate by ids.** Session ids (`session.id=`, `old_session.id=`) and peer
   pubkeys tie the two actors' views of the same connection. `session.open`
   span = dialer side, bare `session.init` = listener side.
3. **Classify before fixing.** Compare against the same test's passing runs and
   against master (`gh run list --branch master --workflow system-tests-nightly.yml`)
   to separate pre-existing flakes from branch regressions.
4. **Reproduce locally only where the harness supports it.** Integration tests
   (in-process, direct connections) cannot reproduce slow network death
   detection; races there need lock barriers or probe gates, never sleeps.

## Pitfalls (learned the hard way)

- **`Connected` floods are not churn.** `connection_monitor` re-emits
  `SessionStatus::Connected` every second for the session's life. Real churn =
  repeated `session_collision_*` / `session_cleaned_up` / `Inactive`.
- **Ghost session signature:** monitor keeps ticking `Connected` while the
  session task logs nothing (no keepalives, no events) — the task is dead or
  stuck while the map entry survives.
- **Substring traps:** `incoming_connection` matches
  `incoming_connection_failed` in log greps; exclude explicitly.
- **Relayed death detection is slow.** A dead peer's relayed connection can
  look alive for tens of seconds (observed ~39s); collision decisions made in
  that window treat the zombie as live (`kept_existing` kills fresh dials).
  Do not assume close propagation is instant, and do not assume in-process
  tests can reproduce it.
- **Timeout arithmetic:** caller `HELLO_TIMEOUT` is 10s; room-goodbye and
  replacement graces are 500ms; offer expiry is 10s. A missing terminal event
  at +10s means the negotiation never reached its read loop — look upstream.
- **Scenario DSL ordering:** `restart_actor` requires `start_manager` and a
  `manager_active Active` wait before further commands; `accept_call` captures
  `request_id` from the last prompt event, and churn can cancel it before the
  accept lands ("unknown accept_call request_id" means exactly that).
- **Setup-phase failures are infrastructure.** `manager_active Active event not
  observed` and `pkarr records not published` on hostile profiles are retried
  automatically (`flaky(only_rerun=...)` in `system-tests/conftest.py`) — don't
  chase them as product bugs.

## Useful References

- `system-tests/harness/scenario.py` — YAML step types (`send`, `expect_event`, `expect_absent`, `concurrent`, `restart_actor`) and variable capture (`${actor.field}`).
- `system-tests/harness/process.py` — CLI event stream mechanics.
- `docs/solutions/runtime-errors/` — documented collision/race fixes with frontmatter tags for search.
