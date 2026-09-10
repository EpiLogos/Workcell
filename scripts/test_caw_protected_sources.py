#!/usr/bin/env python3
"""Native write-boundary regression; requires an explicit product binary.

This exercises Workcell's public operation, not a substitute policy engine. A
platform refusal is distinguished from positive kernel-enforced execution.
"""
from __future__ import annotations
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


class ProtectedSources(unittest.TestCase):
    def setUp(self) -> None:
        configured = os.environ.get("WORKCELL_CAW_BOUNDARY")
        if not configured:
            self.fail("WORKCELL_CAW_BOUNDARY must name the exact native product binary")
        self.binary = Path(configured).resolve(strict=True)
        self.temp = tempfile.TemporaryDirectory(prefix="workcell-protected-source-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "T").mkdir()
        self.source = self.root / "now.json"
        self.source.write_bytes(b"retained native source\r\n")
        self.requirement = {
            "schema": "workcell.write-boundary/v1", "policy_ref": "source:controlled-policy",
            "policy_revision": "revision:controlled-1", "authority_ref": "source:controlled-authority",
            "writable_paths": [str(self.root / "T")], "protected_paths": [str(self.source)],
            "required_coverage": ["file-content", "file-creation", "truncate", "descendant-processes"],
            "expires_at_unix_ms": int(time.time() * 1000) + 300000,
        }
        self.packet = self.root / "requirements.json"

    def invoke(self, *args: str) -> tuple[int, dict]:
        result = subprocess.run([str(self.binary), *args], cwd=self.root,
            env={"PATH": "/usr/bin:/bin", "HOME": str(self.root), "LANG": "C.UTF-8"},
            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=15, check=False)
        return result.returncode, json.loads(result.stdout)

    def inspect(self) -> tuple[int, dict]:
        self.packet.write_text(json.dumps(self.requirement))
        return self.invoke("inspect", str(self.packet), "revision:controlled-1")

    def test_protected_regular_source_retained_through_native_preparation(self) -> None:
        _, caps = self.invoke("capabilities")
        code, result = self.inspect()
        if not caps["supported"]:
            self.assertNotEqual(code, 0)
            self.assertEqual(result["error"], caps["reason"],
                "A valid protected file must reach platform eligibility, not fail directory decoding")
            self.assertNotEqual(os.environ.get("WORKCELL_REQUIRE_LANDLOCK"), "1",
                "Required positive material execution did not run")
            print("PROTECTED_SOURCE_PLATFORM_UNAVAILABLE: no kernel execution claimed")
        else:
            self.assertEqual(code, 0, result)
            self.assertEqual(result["requirements"]["protected_paths"], [str(self.source)])
            self.assertTrue(any(p["path"] == str(self.source) and p["identity"]
                                for p in result["protected_objects"]))
            program = (
                "import sys; from pathlib import Path; p=Path(sys.argv[1]); "
                "Path(sys.argv[2]).write_text('permitted task output');\n"
                "try: p.write_text('escape')\n"
                "except PermissionError: print('protected-source-denied')\n"
                "else: raise AssertionError('source overwritten')\n"
            )
            code, result = self.invoke("run", str(self.packet), "revision:controlled-1", "5000", "--",
                                      sys.executable, "-I", "-c", program,
                                      str(self.source), str(self.root / "T/result.txt"))
            self.assertEqual(code, 0, result)
            self.assertTrue(result["executed"])
            self.assertIn("protected-source-denied", result["stdout"])
            self.assertEqual((self.root / "T/result.txt").read_text(), "permitted task output")
            print("PROTECTED_SOURCE_EXECUTED: native denied source write and permitted T write")
        self.assertEqual(self.source.read_bytes(), b"retained native source\r\n")

    def test_protected_descendant_is_an_unsupported_hole_not_an_omitted_path(self) -> None:
        private = self.root / "T/private.json"
        private.write_text("retained")
        self.requirement["protected_paths"] = [str(private)]
        code, result = self.inspect()
        self.assertNotEqual(code, 0, result)
        self.assertIn("writable directory includes a protected object", result["error"])
        self.assertIsNone(result["executed"])
        self.assertEqual(private.read_text(), "retained")

    def test_regular_file_does_not_become_a_writable_directory_grant(self) -> None:
        self.requirement["writable_paths"] = [str(self.source)]
        self.requirement["protected_paths"] = []
        code, result = self.inspect()
        self.assertNotEqual(code, 0, result)
        self.assertIsNone(result["executed"])
        self.assertEqual(self.source.read_bytes(), b"retained native source\r\n")


if __name__ == "__main__":
    unittest.main(verbosity=2)
