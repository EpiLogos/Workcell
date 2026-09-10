"""Real native protocol exec: no wrapper reply, same PID, kernel-protected writes."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

BODY = r'''
import json, os, pathlib, sys
for line in sys.stdin:
    request = json.loads(line)
    path = pathlib.Path(request['path'])
    try:
        path.write_text('CONTROLLED_PROTOCOL_EFFECT')
        allowed = True
    except PermissionError:
        allowed = False
    print(json.dumps({'id': request['id'], 'pid': os.getpid(), 'allowed': allowed}), flush=True)
'''

class NativeProtocolBoundary(unittest.TestCase):
    def setUp(self):
        self.binary = os.environ.get('WORKCELL_CAW_BOUNDARY')
        if not self.binary:
            if os.environ.get('WORKCELL_REQUIRE_LANDLOCK') == '1':
                self.fail('mandatory native protocol proof has no exact executable')
            self.skipTest('requires source-built workcell-write-boundary')
        capabilities = subprocess.run([self.binary, 'capabilities'], capture_output=True, text=True, check=True)
        caps = json.loads(capabilities.stdout)
        if not caps['supported']:
            if os.environ.get('WORKCELL_REQUIRE_LANDLOCK') == '1':
                self.fail('mandatory positive Landlock proof unavailable: ' + str(caps))
            self.skipTest('no positive Landlock in this runtime')
        self.assertTrue(caps.get('protocol_exec'), caps)
        self.temp = tempfile.TemporaryDirectory(prefix='workcell-protocol-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.now = self.root / 'T'
        self.now.mkdir()
        self.source = self.root / 'human.txt'
        self.source.write_text('HUMAN_BYTES')
        self.req = {'schema': 'workcell.write-boundary/v1', 'policy_ref': 'policy:controlled',
                    'policy_revision': 'revision:1', 'authority_ref': 'authority:controlled',
                    'writable_paths': [str(self.now)], 'protected_paths': [str(self.source)],
                    'required_coverage': ['file-content', 'file-creation', 'descendant-processes'],
                    'expires_at_unix_ms': int(time.time() * 1000) + 60000}
        self.requirements = self.root / 'requirements.json'
        self.requirements.write_text(json.dumps(self.req))
        inspected = subprocess.run([self.binary, 'inspect', str(self.requirements), 'revision:1'],
                                   capture_output=True, text=True, check=True)
        self.preparation = json.loads(inspected.stdout)
        self.path = self.root / 'preparation.json'
        self.path.write_text(json.dumps(self.preparation))

    def argv(self, revision='revision:1'):
        return [self.binary, 'exec', str(self.path), revision, '--', sys.executable, '-u', '-c', BODY]

    def test_two_turns_use_same_process_and_cannot_write_protected_source(self):
        process = subprocess.Popen(self.argv(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        packets = [{'id': 1, 'path': str(self.now / 'result.txt')},
                   {'id': 2, 'path': str(self.source)},
                   {'id': 3, 'path': str(self.root / 'loose.txt')}]
        out, err = process.communicate(''.join(json.dumps(p) + '\n' for p in packets), timeout=10)
        self.assertEqual(process.returncode, 0, err)
        replies = [json.loads(line) for line in out.splitlines()]
        self.assertEqual([r['id'] for r in replies], [1, 2, 3])
        self.assertEqual([r['pid'] for r in replies], [process.pid] * 3)
        self.assertEqual([r['allowed'] for r in replies], [True, False, False])
        self.assertEqual(self.source.read_text(), 'HUMAN_BYTES')
        self.assertEqual((self.now / 'result.txt').read_text(), 'CONTROLLED_PROTOCOL_EFFECT')
        self.assertFalse((self.root / 'loose.txt').exists())
        print('PROTECTED_PROTOCOL_EXECUTED: same PID, native stdio, actual permitted/denied writes')

    def test_replaced_material_object_requires_new_preparation(self):
        self.source.rename(self.root / 'retained-human.txt')
        self.source.write_text('REPLACEMENT_HUMAN_BYTES')
        result = subprocess.run(self.argv(), input='', capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, '')
        self.assertIn('protected_objects changed', result.stderr)
        self.assertEqual(self.source.read_text(), 'REPLACEMENT_HUMAN_BYTES')

    def test_preopened_regular_file_is_not_a_protocol_channel(self):
        log = self.root / 'preopened.txt'
        with log.open('w') as output:
            result = subprocess.run(self.argv(), input='{}\n', stdout=output,
                                    stderr=subprocess.PIPE, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(log.read_bytes(), b'')
        self.assertIn('pipe or socket', result.stderr)
        self.assertEqual(self.source.read_text(), 'HUMAN_BYTES')

    def test_stale_and_expired_requests_never_execute_or_pollute_stdout(self):
        for expired in (False, True):
            with self.subTest(expired=expired):
                if expired:
                    self.preparation['requirements']['expires_at_unix_ms'] = 1
                    self.path.write_text(json.dumps(self.preparation))
                result = subprocess.run(self.argv('revision:1' if expired else 'wrong'),
                    input=json.dumps({'id': 1, 'path': str(self.now / 'forbidden.txt')}) + '\n',
                    capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, '')
                self.assertFalse((self.now / 'forbidden.txt').exists())
                self.assertFalse(json.loads(result.stderr)['executed'])

if __name__ == '__main__':
    unittest.main()
