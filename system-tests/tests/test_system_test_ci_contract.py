from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).parents[2]


def test_given_system_workflow_when_read_then_it_uses_only_namespace_entrypoint() -> None:
    workflow = (ROOT / ".github" / "workflows" / "system-tests.yml").read_text(
        encoding="utf-8"
    )

    assert "system-tests/run-in-user-namespace.sh python -m pytest" in workflow
    assert "libasound2-dev" in workflow
    assert "iproute2" in workflow
    assert "timeout-minutes: 40" in workflow
    assert "fail-fast: false" in workflow
    assert "sudo" not in workflow
    assert "docker" not in workflow.casefold()
    assert "compose" not in workflow.casefold()


def test_given_sweep_callers_when_read_then_pr_uses_three_and_nightly_uses_ten() -> None:
    ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    nightly = (ROOT / ".github" / "workflows" / "system-tests-nightly.yml").read_text(
        encoding="utf-8"
    )

    assert "sweep_indices: '[0,1,2]'" in ci
    assert "sweep_indices: '[0,1,2,3,4,5,6,7,8,9]'" in nightly


def test_given_legacy_compose_paths_when_migrated_then_files_are_removed() -> None:
    system_tests = ROOT / "system-tests"

    assert not (system_tests / "docker-compose.yml").exists()
    assert not (system_tests / "up.sh").exists()
    assert not (system_tests / "down.sh").exists()
