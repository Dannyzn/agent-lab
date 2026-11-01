#!/usr/bin/env python3
"""Deterministic real-CLI approval, Seatbelt and signal acceptance tests (macOS)."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import ThreadingHTTPServer

LAB = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(LAB / 'contracts'))
from mock_responses_server import State, handler_for  # noqa: E402

ARTIFACTS = LAB / 'experiments/phase2/artifacts'


def call(script, name='exec_command', arguments=None):
    return {'tool_calls': [{'call_id': 'call-1', 'name': name,
                            'arguments': arguments or {'argv': ['python3', '-c', script], 'timeout_ms': 10000}}]}


class Run:
    def __init__(self, responses, options=(), hold_model=False, human=False):
        self.root = Path(tempfile.mkdtemp(prefix='run-', dir=str(ARTIFACTS)))
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        self.requests = self.root / 'requests.jsonl'
        self.events_path = self.root / 'events.jsonl'
        self.stdout_path = self.root / 'stdout.txt'
        self.stderr_path = self.root / 'stderr.txt'
        self.release = threading.Event()
        self.model_received = threading.Event()
        state = State({'responses': responses}, self.requests)
        if hold_model:
            next_response = state.next
            def wait_next(request):
                self.model_received.set()
                self.release.wait(5)
                return next_response(request)
            state.next = wait_next
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), handler_for(state))
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.stdout = self.stdout_path.open('w')
        self.stderr = self.stderr_path.open('w')
        argv = [str(LAB / 'target/debug/our-agent'), '--workspace', str(self.workspace),
                '--base-url', 'http://127.0.0.1:{}/v1'.format(self.server.server_port),
                '--model', 'phase2-contract', '--events', str(self.events_path)]
        if not human:
            argv.append('--json')
        argv.extend(options)
        argv.append('Complete the controlled acceptance task.')
        self.process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=self.stdout, stderr=self.stderr,
                                        env=dict(os.environ, OPENAI_API_KEY='phase2-contract-key'), text=True)

    def events(self):
        if not self.events_path.exists():
            return []
        return [json.loads(line) for line in self.events_path.read_text().split('\n')[:-1] if line.strip()]

    def wait_for(self, predicate, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            found = next((event for event in self.events() if predicate(event)), None)
            if found is not None:
                return found
            if self.process.poll() is not None:
                raise AssertionError('CLI exited before expected event: ' + self.stderr_path.read_text())
            time.sleep(0.01)
        raise AssertionError('timed out waiting for CLI event; artifacts: ' + str(self.root))

    def reply(self, approval_id, decision):
        self.process.stdin.write(json.dumps({'approval_id': approval_id, 'decision': decision}) + '\n')
        self.process.stdin.flush()

    def finish(self, expected=0):
        self.process.wait(timeout=5)
        if self.process.returncode != expected:
            raise AssertionError('exit={} expected={}; {}'.format(self.process.returncode, expected, self.stderr_path.read_text()))
        return self.events()

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=3)
        self.process.stdin.close()
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)
        self.stdout.close()
        self.stderr.close()


class Phase2CLI(unittest.TestCase):
    def start(self, responses, options=(), **kwargs):
        run = Run(responses, options, **kwargs)
        self.addCleanup(run.close)
        return run

    def test_approval_allow_executes_once_and_preserves_history(self):
        run = self.start([call("import os; from pathlib import Path; assert 'OPENAI_API_KEY' not in os.environ; Path('approved').write_text('yes')"), {'text': 'done'}])
        request = run.wait_for(lambda e: e['type'] == 'approval.requested')
        self.assertFalse((run.workspace / 'approved').exists())
        self.assertEqual(request['capabilities']['sandbox']['network'], 'deny')
        run.reply(request['approval_id'], 'allow')
        events = run.finish()
        self.assertTrue((run.workspace / 'approved').exists())
        self.assertEqual(events[-1]['type'], 'turn.completed')
        requests = [json.loads(line)['body'] for line in run.requests.read_text().splitlines()]
        self.assertEqual([len(r['input']) for r in requests], [1, 3])
        self.assertEqual(len([e for e in events if e['type'] == 'approval.resolved']), 1)
        self.assertEqual(events, [json.loads(line) for line in run.stdout_path.read_text().splitlines()])

    def test_approval_deny_and_mismatched_id_cannot_write(self):
        for decision, wrong_id in [('deny', False), ('allow', True)]:
            with self.subTest(decision=decision, wrong_id=wrong_id):
                run = self.start([call("from pathlib import Path; Path('denied').write_text('bad')"), {'text': 'stopped'}])
                request = run.wait_for(lambda e: e['type'] == 'approval.requested')
                run.reply('wrong-id' if wrong_id else request['approval_id'], decision)
                events = run.finish()
                self.assertFalse((run.workspace / 'denied').exists())
                item = next(e['item'] for e in events if e['type'] == 'item.completed')
                self.assertFalse(item['result']['success'])

    def test_approval_eof_denies(self):
        run = self.start([call("from pathlib import Path; Path('denied').write_text('bad')"), {'text': 'done'}])
        run.wait_for(lambda e: e['type'] == 'approval.requested')
        run.process.stdin.close()
        events = run.finish()
        self.assertFalse((run.workspace / 'denied').exists())
        self.assertTrue(any(e['type'] == 'approval.resolved' and e['decision'] == 'deny' for e in events))

    def test_never_denies_without_prompt(self):
        run = self.start([call("from pathlib import Path; Path('denied').write_text('bad')"), {'text': 'done'}], ['--approval', 'never'])
        events = run.finish()
        self.assertFalse((run.workspace / 'denied').exists())
        self.assertFalse(any(e['type'] == 'approval.requested' for e in events))

    def test_unlisted_command_is_denied_even_in_auto(self):
        run = self.start([call('', arguments={'argv': ['sh', '-c', 'touch denied']}), {'text': 'done'}], ['--approval', 'auto'])
        events = run.finish()
        self.assertFalse((run.workspace / 'denied').exists())
        self.assertIn('not allowed', next(e['item'] for e in events if e['type'] == 'item.completed')['result']['output'])

    def test_readonly_blocks_patch_before_approval(self):
        patch = '*** Begin Patch\n*** Update File: value.txt\n@@\n-old\n+new\n*** End Patch'
        run = self.start([call('', name='apply_patch', arguments={'patch': patch}), {'text': 'done'}], ['--approval', 'auto', '--filesystem', 'read-only'])
        (run.workspace / 'value.txt').write_text('old\n')
        events = run.finish()
        self.assertEqual((run.workspace / 'value.txt').read_text(), 'old\n')
        self.assertFalse(any(e['type'] == 'approval.requested' for e in events))

    def test_seatbelt_blocks_external_write_after_approval(self):
        # The sibling directory is accessible to the host, outside tool workspace.
        run = self.start([call("from pathlib import Path; Path('../escape').write_text('bad')"), {'text': 'done'}])
        request = run.wait_for(lambda e: e['type'] == 'approval.requested')
        run.reply(request['approval_id'], 'allow')
        events = run.finish()
        self.assertFalse((run.root / 'escape').exists())
        result = next(e['item']['result'] for e in events if e['type'] == 'item.completed')
        self.assertFalse(result['success'])
        self.assertIn('PermissionError', result['output'])

    def test_cancel_during_approval_has_no_side_effect(self):
        run = self.start([call("from pathlib import Path; Path('late').write_text('bad')")])
        run.wait_for(lambda e: e['type'] == 'approval.requested')
        run.process.send_signal(signal.SIGINT)
        events = run.finish(expected=130)
        self.assertFalse((run.workspace / 'late').exists())
        self.assertEqual(events[-1]['type'], 'turn.cancelled')
        self.assertTrue(any(e['type'] == 'approval.resolved' and e['decision'] == 'cancelled' for e in events))
        self.assertEqual(sum(e['type'].startswith('turn.') and e['type'] != 'turn.started' for e in events), 1)

    def test_cancel_during_model_wait_exits_promptly(self):
        run = self.start([{'text': 'late'}], hold_model=True)
        self.assertTrue(run.model_received.wait(timeout=3))
        run.process.send_signal(signal.SIGTERM)
        self.assertEqual(run.finish(expected=130)[-1]['type'], 'turn.cancelled')

    def test_cancel_exec_cleans_child_before_terminal_event(self):
        script = "import pathlib,subprocess,time; subprocess.Popen(['/usr/bin/python3','-c',\"import pathlib,time; time.sleep(1); pathlib.Path('leaked').write_text('bad')\"]); pathlib.Path('ready').write_text('yes'); time.sleep(10)"
        run = self.start([call(script)], ['--approval', 'auto'])
        deadline = time.monotonic() + 5
        while not (run.workspace / 'ready').exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue((run.workspace / 'ready').exists())
        run.process.send_signal(signal.SIGINT)
        events = run.finish(expected=130)
        time.sleep(1.2)
        self.assertFalse((run.workspace / 'leaked').exists())
        self.assertEqual(events[-1]['type'], 'turn.cancelled')
        self.assertTrue(any(e['type'] == 'item.completed' and e['item']['status'] == 'cancelled' for e in events))

    def test_approval_timeout_terminates_turn(self):
        run = self.start([call("print('must not execute')")], ['--approval-timeout-ms', '150'])
        run.wait_for(lambda e: e['type'] == 'approval.requested')
        events = run.finish(expected=1)
        self.assertEqual(events[-1]['type'], 'turn.failed')
        self.assertEqual(len([e for e in events if e['type'] == 'approval.resolved']), 1)

    def test_log_inside_tool_workspace_is_rejected(self):
        root = Path(tempfile.mkdtemp(prefix='log-boundary-', dir=str(ARTIFACTS)))
        process = subprocess.run([str(LAB / 'target/debug/our-agent'), '--workspace', str(root),
                                  '--events', str(root / 'events.jsonl'), 'must not run'],
                                 capture_output=True, text=True, timeout=3)
        self.assertEqual(process.returncode, 1)
        self.assertIn('event log must be outside', process.stderr)
        self.assertFalse((root / 'events.jsonl').exists())

    def test_human_approval_accepts_yes(self):
        run = self.start([call("print('approved')"), {'text': 'human done'}], human=True)
        run.wait_for(lambda e: e['type'] == 'approval.requested')
        run.process.stdin.write('y\n')
        run.process.stdin.flush()
        run.finish()
        self.assertIn('human done', run.stdout_path.read_text())
        self.assertIn('Allow this call once?', run.stderr_path.read_text())


if __name__ == '__main__':
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Phase2CLI)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    (ARTIFACTS / 'summary.json').write_text(json.dumps({'tests': result.testsRun, 'failures': len(result.failures), 'errors': len(result.errors), 'success': result.wasSuccessful()}, indent=2) + '\n')
    sys.exit(0 if result.wasSuccessful() else 1)
