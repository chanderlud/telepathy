from __future__ import annotations

from harness.process import _normalize_command_payload


def test_unit_command_drops_empty_args() -> None:
    payload = {"cmd": "start_manager", "args": {}}

    normalized = _normalize_command_payload(payload)

    assert normalized == {"cmd": "start_manager"}


def test_non_unit_command_keeps_default_args_object() -> None:
    payload = {"cmd": "start_session"}

    normalized = _normalize_command_payload(payload)

    assert normalized["cmd"] == "start_session"
    assert normalized["args"] == {}


def test_add_contact_legacy_fields_are_mapped() -> None:
    payload = {
        "cmd": "add_contact",
        "args": {"contact_id": "bob", "peer_id": "peer-123"},
    }

    normalized = _normalize_command_payload(payload)

    assert normalized["cmd"] == "add_contact"
    assert normalized["args"] == {
        "id": "bob",
        "nickname": "bob",
        "peer_id": "peer-123",
    }
