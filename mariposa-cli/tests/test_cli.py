"""CLI subcommands over the same fake /oqueues tree (OQ_TRANSPORT=local).

Drives the umbrella entry point exactly as a user would: `mariposa oqueues …`.
"""

import json

from mariposa_cli import cli


def test_cli_list(fake_oqfs, capsys):
    assert cli.main(["oqueues", "list"]) == 0
    queues = json.loads(capsys.readouterr().out)
    assert {q["name"] for q in queues} == {"scheduler.events", "raid1.rebuilds"}


def test_cli_metadata(fake_oqfs, capsys):
    assert cli.main(["oqueues", "metadata", "scheduler/events"]) == 0
    assert "scheduler.events" in capsys.readouterr().out


def test_cli_collect_max_records_json(fake_oqfs, capsys):
    assert (
        cli.main(
            [
                "oqueues",
                "collect",
                "scheduler/events",
                "--max-records",
                "2",
                "--format",
                "json",
            ]
        )
        == 0
    )
    records = json.loads(capsys.readouterr().out)
    assert len(records) == 2
    assert records[0]["task"] == 1


def test_cli_collect_requires_bound(fake_oqfs, capsys):
    # No --max-records/--timeout: collect must refuse rather than hang.
    assert cli.main(["oqueues", "collect", "scheduler/events"]) == 1
    assert "requires max_records or timeout_s" in capsys.readouterr().err


def test_cli_unknown_stream_reports_error(fake_oqfs, capsys):
    # A guest command failure surfaces as a clean nonzero exit, not a traceback.
    assert cli.main(["oqueues", "metadata", "does/not/exist"]) == 1
    assert "error:" in capsys.readouterr().err
