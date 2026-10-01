"""Native prepared protocol exec: exact objects, no wrapper reply, same PID."""
import json
import os
from pathlib import Path
import socket
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

    def argv(self, revision='revision:1', body=BODY):
        return [self.binary, 'exec', str(self.path), revision,
                self.preparation['requirements_digest'], '--', sys.executable, '-u', '-c', body]

    def test_failed_provider_retains_diagnostics_without_polluting_protocol_stdout(self):
        body = r'''
import json, os
print(json.dumps({'pid': os.getpid(), 'protocol': 'ready'}), flush=True)
raise RuntimeError('CONTROLLED_NATIVE_PROVIDER_FAILURE')
'''
        process = subprocess.Popen(self.argv(body=body), stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        out, err = process.communicate('', timeout=10)
        self.assertEqual(process.returncode, 1)
        self.assertEqual(json.loads(out), {'pid': process.pid, 'protocol': 'ready'})
        self.assertIn('RuntimeError: CONTROLLED_NATIVE_PROVIDER_FAILURE', err)
        self.assertNotIn('CONTROLLED_NATIVE_PROVIDER_FAILURE', out)

    def test_socket_diagnostic_channel_retains_actual_provider_bytes(self):
        reader, writer = socket.socketpair()
        self.addCleanup(reader.close)
        self.addCleanup(writer.close)
        reader.settimeout(10)
        body = "import sys; sys.stderr.write('CONTROLLED_SOCKET_DIAGNOSTIC\\n'); sys.exit(23)"
        process = subprocess.Popen(self.argv(body=body), stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=writer)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        writer.close()
        out, _ = process.communicate(b'', timeout=10)
        self.assertEqual(process.returncode, 23)
        self.assertEqual(out, b'')
        diagnostics = []
        while chunk := reader.recv(4096):
            diagnostics.append(chunk)
        self.assertEqual(b''.join(diagnostics), b'CONTROLLED_SOCKET_DIAGNOSTIC\n')

    def test_regular_file_stderr_refuses_provider_execution(self):
        diagnostic_file = self.root / 'preopened-stderr.txt'
        marker = self.now / 'must-not-run.txt'
        body = "import pathlib, sys; pathlib.Path(%r).write_text('FORBIDDEN_EXECUTION'); sys.stderr.write('FORBIDDEN_PROVIDER_DIAGNOSTIC')" % str(marker)
        with diagnostic_file.open('w') as diagnostic:
            result = subprocess.run(self.argv(body=body), input='', stdout=subprocess.PIPE,
                                    stderr=diagnostic, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, '')
        self.assertFalse(marker.exists())
        refusal = json.loads(diagnostic_file.read_text())
        self.assertFalse(refusal['executed'])
        self.assertIn('pipe/socket', refusal['error'])
        self.assertNotIn('FORBIDDEN_PROVIDER_DIAGNOSTIC', diagnostic_file.read_text())

    def test_nonstdio_file_and_socket_handles_are_closed_before_provider_exec(self):
        inherited_file = self.root / 'inherited.txt'
        inherited_file.write_text('RETAINED_BYTES')
        reader, writer = socket.socketpair()
        self.addCleanup(reader.close)
        self.addCleanup(writer.close)
        with inherited_file.open('a') as extra:
            fds = [extra.fileno(), writer.fileno()]
            body = r'''
import errno, json, os
closed = []
for fd in %r:
    try:
        os.write(fd, b'FORBIDDEN_INHERITED_EFFECT')
        closed.append(False)
    except OSError as error:
        closed.append(error.errno == errno.EBADF)
print(json.dumps({'pid': os.getpid(), 'closed': closed}), flush=True)
''' % fds
            process = subprocess.Popen(self.argv(body=body), stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       pass_fds=fds, text=True)
            self.addCleanup(lambda: process.kill() if process.poll() is None else None)
            out, err = process.communicate('', timeout=10)
        self.assertEqual(process.returncode, 0, err)
        self.assertEqual(json.loads(out), {'pid': process.pid, 'closed': [True, True]})
        self.assertEqual(inherited_file.read_text(), 'RETAINED_BYTES')
        writer.close()
        reader.settimeout(10)
        self.assertEqual(reader.recv(1), b'')

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
        self.assertIn('pipe/socket', result.stderr)
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
