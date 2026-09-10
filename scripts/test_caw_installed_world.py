#!/usr/bin/env python3
"""Procedure safety regression tests, not installed-world execution evidence."""
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import caw_installed_world as installed
from caw_native_campaign import demand


class InstalledProcedureSafety(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="workcell-procedure-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.packet = {"schema": "workcell.installed-campaign/v1", "authorized_material_effects": True,
                       "test_world": True, "release_after_test": True, "demand": demand(), "placements": []}
        for number in (1, 2):
            census = self.root / f"census-{number}.json"
            census.write_text(json.dumps({"schema": "workcell.installed-census/v1", "host_identity_sha256": str(number)*64}))
            self.packet["placements"].append({"workcell_ref": f"workcell:{number}", "endpoint": f"tcp://127.0.0.1:{12000+number}",
                "census_file": str(census), "token_env": "WORKCELL_TEST_TOKEN", "authorized_restart_argv": ["true"]})
        self.args = SimpleNamespace(manifest=self.root/"packet.json", output=self.root/"result.json", execute_authorized=True)

    def write_packet(self):
        self.args.manifest.write_text(json.dumps(self.packet))

    def test_existing_evidence_refuses_before_any_effect(self):
        self.args.output.write_text("preserved")
        with patch.object(installed, "exercise") as run:
            with self.assertRaises(FileExistsError):
                installed.record_exercise(self.args)
            run.assert_not_called()
        self.assertEqual(self.args.output.read_text(), "preserved")

    def test_interruption_preserves_append_only_partial_receipt(self):
        def interrupted(args, evidence, checkpoint):
            evidence.update(status="prepared", world_ref="world:actual-observation-fixture")
            checkpoint()
            raise KeyboardInterrupt()
        with patch.object(installed, "exercise", interrupted):
            with self.assertRaises(KeyboardInterrupt):
                installed.record_exercise(self.args)
        lines = [json.loads(line) for line in Path(str(self.args.output)+".journal.jsonl").read_text().splitlines()]
        self.assertEqual(lines[0]["status"], "not-started")
        self.assertEqual(lines[1]["world_ref"], "world:actual-observation-fixture")
        self.assertEqual(lines[-1]["error_type"], "KeyboardInterrupt")
        self.assertIn("failed-or-interrupted", json.loads(self.args.output.read_text())["status"])
        if os.name == "posix":
            self.assertEqual(self.args.output.stat().st_mode & 0o777, 0o600)

    def test_unauthorised_packet_refuses_without_network(self):
        self.packet["authorized_material_effects"] = False
        self.write_packet()
        with patch.object(installed, "request") as request:
            with self.assertRaisesRegex(RuntimeError, "authorised"):
                installed.exercise(self.args, {})
            request.assert_not_called()

    def test_same_host_census_cannot_be_second_placement(self):
        self.packet["placements"][1]["census_file"] = self.packet["placements"][0]["census_file"]
        self.write_packet()
        with patch.object(installed, "request") as request:
            with self.assertRaisesRegex(RuntimeError, "same-host"):
                installed.exercise(self.args, {})
            request.assert_not_called()

    def test_missing_second_restart_refuses_before_prepare(self):
        self.packet["placements"][1]["authorized_restart_argv"] = []
        self.write_packet()
        def read_only(address, operation, payload=None, **kwargs):
            if operation == "discover":
                return {"workcell_ref": "workcell:1"}
            if operation == "plan":
                return {"status": "satisfiable"}
            self.fail("preflight performed a material effect")
        with patch.dict(os.environ, {"WORKCELL_TEST_TOKEN": "test-only"}), patch.object(installed, "request", side_effect=read_only):
            with self.assertRaisesRegex(RuntimeError, "restart argv"):
                installed.exercise(self.args, {})

    def test_missing_release_choice_refuses(self):
        del self.packet["release_after_test"]
        self.write_packet()
        with self.assertRaisesRegex(RuntimeError, "release_after_test"):
            installed.exercise(self.args, {})

    def test_credentials_in_endpoint_refused(self):
        with self.assertRaisesRegex(RuntimeError, "credentials"):
            installed.address("tcp://user:secret@127.0.0.1:1234")

    def test_empty_demand_cannot_claim_installed_hosting(self):
        self.packet["demand"]["storage"]["required"] = []
        self.write_packet()
        with self.assertRaisesRegex(RuntimeError, "service and NOW"):
            installed.exercise(self.args, {})


if __name__ == "__main__":
    unittest.main()
