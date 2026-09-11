"""Native stdio preservation through the real Workcell boundary, not a mock."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


class ProtocolBoundary(unittest.TestCase):
    def test_native_protocol_and_enforcement_or_explicit_unsupported(self):
        binary = os.environ["WORKCELL_CAW_BOUNDARY"]
        cap = json.loads(subprocess.check_output([binary, "capabilities"], text=True))
        with tempfile.TemporaryDirectory(prefix="workcell-protocol-") as name:
            root = Path(name)
            writable = root / "T"
            writable.mkdir()
            human = root / "source.json"
            human.write_text("HUMAN")
            req = {"schema": "workcell.write-boundary/v1", "policy_ref": "source:policy",
                   "policy_revision": "revision:1", "authority_ref": "authority:admitted",
                   "writable_paths": [str(writable)], "protected_paths": [str(human)],
                   "required_coverage": ["file-content", "file-creation", "descendant-processes"],
                   "expires_at_unix_ms": int(time.time() * 1000) + 120000}
            path = root / "requirements.json"
            path.write_text(json.dumps(req))
            inspection = subprocess.run([binary, "inspect", str(path), "revision:1"],
                                        capture_output=True, text=True, timeout=10)
            if not cap["supported"]:
                self.assertNotEqual(inspection.returncode, 0)
                self.assertNotEqual(os.environ.get("WORKCELL_REQUIRE_LANDLOCK"), "1")
                return
            self.assertEqual(inspection.returncode, 0, inspection.stdout)
            digest = json.loads(inspection.stdout)["requirements_digest"]
            script = """
import json, os, sys
from pathlib import Path
r=Path(sys.argv[1]); request=json.loads(sys.stdin.readline())
try: (r/'source.json').write_text('FORBIDDEN')
except PermissionError: denied=True
else: denied=False
(r/'T'/'result').write_text(request['payload'])
print(json.dumps({'reply':request,'denied':denied,'pid':os.getpid()}),flush=True)
"""
            argv = [binary, "exec", str(path), "revision:1", digest, "--",
                    "/usr/bin/python3", "-S", "-c", script, str(root)]
            result = subprocess.run(argv, input='{"payload":"ACTUAL_PROTOCOL"}\n',
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(result.stdout.splitlines()), 1)
            reply = json.loads(result.stdout)
            self.assertTrue(reply["denied"])
            self.assertEqual(reply["reply"]["payload"], "ACTUAL_PROTOCOL")
            self.assertEqual((writable / "result").read_text(), "ACTUAL_PROTOCOL")
            self.assertEqual(human.read_text(), "HUMAN")
            for change in ("digest", "expiry", "inherited-file"):
                negative = argv.copy()
                if change == "digest":
                    negative[4] = "sha256:wrong"
                if change == "expiry":
                    req["expires_at_unix_ms"] = 1
                    path.write_text(json.dumps(req))
                if change == "inherited-file":
                    req["expires_at_unix_ms"] = int(time.time() * 1000) + 120000
                    path.write_text(json.dumps(req))
                    fresh = json.loads(subprocess.check_output([binary, "inspect", str(path), "revision:1"], text=True))
                    negative[4] = fresh["requirements_digest"]
                    with human.open("rb") as source:
                        denied = subprocess.run(negative, stdin=source, capture_output=True, text=True, timeout=10)
                else:
                    denied = subprocess.run(negative, input="{}\n", capture_output=True, text=True, timeout=10)
                self.assertNotEqual(denied.returncode, 0, change)
                self.assertEqual(denied.stdout, "", change)
            print("STDIO_BOUNDARY_EXECUTED", flush=True)
