#!/usr/bin/env python3
"""Real CLI + loopback SSE + durable WAL. No cloud service or credential needed."""
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
from mock_responses_server import State, handler_for

ARTIFACTS = LAB / 'experiments/phase3/artifacts'


def call(ident='c1', script="from pathlib import Path; p=Path('count'); p.write_text(p.read_text()+'x' if p.exists() else 'x')", name='exec_command', arguments=None):
    return {'tool_calls': [{'call_id': ident, 'name': name, 'arguments': arguments or {'argv': ['python3', '-c', script], 'timeout_ms': 10000}}]}


def lines(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().split('\n')[:-1]]


def wait_for(predicate, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.01)
    raise AssertionError('timed out waiting for test condition')


class Lab:
    def __init__(self, responses, options=(), hold_index=None):
        self.root = Path(tempfile.mkdtemp(prefix='run-', dir=ARTIFACTS))
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        self.session = self.root / 'session.jsonl'
        self.events_path = self.root / 'events.jsonl'
        self.requests_path = self.root / 'requests.jsonl'
        self.options = list(options)
        self.release = threading.Event()
        self.received = threading.Event()
        self.processes = []
        self.handles = []
        self.state = State({'responses': responses}, self.requests_path)
        next_response = self.state.next

        def next_request(request):
            index = self.state.index
            response = next_response(request)
            if index == hold_index:
                self.received.set()
                self.release.wait(15)
            return response
        self.state.next = next_request
        handler = handler_for(self.state)
        original = handler.do_POST
        def handle(handler_self):
            try:
                original(handler_self)
            except (BrokenPipeError, ConnectionResetError):
                pass
        handler.do_POST = handle
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def launch(self, resume=False, prompt=None, extra=(), workspace=True):
        index = len(self.processes)
        stdout = (self.root / f'stdout-{index}.jsonl').open('w')
        stderr = (self.root / f'stderr-{index}.txt').open('w')
        self.handles.extend([stdout, stderr])
        argv = [str(LAB / 'target/debug/our-agent'), '--base-url', f'http://127.0.0.1:{self.server.server_port}/v1',
                '--model', 'contract-model', '--json', '--events', str(self.events_path),
                '--resume' if resume else '--session', str(self.session)]
        if workspace:
            argv.extend(['--workspace', str(self.workspace)])
        argv.extend(self.options)
        argv.extend(extra)
        if prompt is not None:
            argv.append(prompt)
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr, text=True,
                                   env=dict(os.environ, OPENAI_API_KEY='contract-test-key'))
        process.test_stderr = self.root / f'stderr-{index}.txt'
        self.processes.append(process)
        return process

    def finish(self, process, expected=0):
        process.wait(timeout=7)
        assert process.returncode == expected, f'exit={process.returncode}; {process.test_stderr.read_text()}'
        return self.events()

    def events(self):
        return lines(self.events_path)

    def records(self):
        return [line['record'] for line in lines(self.session)]

    def requests(self):
        return [line['body'] for line in lines(self.requests_path)]

    def event(self, kind, count=1):
        return wait_for(lambda: ([e for e in self.events() if e['type'] == kind] or [None])[count-1]
                        if len([e for e in self.events() if e['type'] == kind]) >= count else None)

    def kill(self, process):
        process.kill()
        process.wait(timeout=3)

    def reply(self, process, approval_id):
        process.stdin.write(json.dumps({'approval_id': approval_id, 'decision': 'allow'}) + '\n')
        process.stdin.flush()

    def close(self):
        for p in self.processes:
            if p.poll() is None:
                p.kill()
                p.wait(timeout=3)
            p.stdin.close()
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)
        for handle in self.handles:
            handle.close()


class Phase3CLI(unittest.TestCase):
    def start(self, responses, options=(), **kwargs):
        lab = Lab(responses, options, **kwargs)
        self.addCleanup(lab.close)
        return lab

    def test_completed_resume_is_cached_and_followup_reuses_thread(self):
        lab = self.start([call(), {'text': 'first answer'}, {'text': 'followup answer'}], ['--approval', 'auto'])
        lab.finish(lab.launch(prompt='first task'))
        self.assertEqual(lab.session.stat().st_mode & 0o777, 0o600)
        self.assertNotIn('contract-test-key', lab.session.read_text())
        lab.finish(lab.launch(resume=True, workspace=False))
        self.assertEqual(len(lab.requests()), 2)
        self.assertEqual((lab.workspace / 'count').read_text(), 'x')
        lab.finish(lab.launch(resume=True, prompt='followup', workspace=False))
        self.assertEqual(len({e['thread_id'] for e in lab.events()}), 1)
        self.assertEqual(sum(e['type'] == 'turn.started' for e in lab.events()), 2)
        history = lab.requests()[-1]['input']
        self.assertTrue(any(i.get('role') == 'assistant' and i['content'][0]['text'] == 'first answer' for i in history))
        self.assertEqual(history[-1]['content'][0]['text'], 'followup')
        ids = [e['event_id'] for e in lab.events()]
        self.assertEqual(len(ids), len(set(ids)))

    def test_killed_approval_gets_fresh_approval_without_resampling(self):
        lab = self.start([call(), {'text': 'done'}])
        process = lab.launch(prompt='increment once')
        first = lab.event('approval.requested')
        self.assertFalse((lab.workspace / 'count').exists())
        lab.kill(process)
        process = lab.launch(resume=True)
        second = lab.event('approval.requested', 2)
        self.assertNotEqual(first['approval_id'], second['approval_id'])
        self.assertTrue(any(e['type'] == 'approval.resolved' and e['approval_id'] == first['approval_id'] and e['decision'] == 'cancelled' for e in lab.events()))
        lab.reply(process, second['approval_id'])
        lab.finish(process)
        self.assertEqual((lab.workspace / 'count').read_text(), 'x')
        self.assertEqual(len(lab.requests()), 2)
        self.assertEqual(sum(e['type'] == 'turn.started' for e in lab.events()), 1)

    def test_killed_started_command_is_unknown_and_never_replayed(self):
        # The child exits by itself before resuming; SIGKILL cannot promise process-group cleanup.
        script = "import time; from pathlib import Path; p=Path('count'); p.write_text(p.read_text()+'x' if p.exists() else 'x'); time.sleep(.6); Path('child-exited').write_text('yes')"
        batch = call(script=script)
        batch['tool_calls'] += call('dependent', script="from pathlib import Path; Path('must-not-execute').write_text('bad')")['tool_calls']
        lab = self.start([batch, {'text': 'inspect unknown result'}], ['--approval', 'auto'])
        process = lab.launch(prompt='increment once')
        wait_for(lambda: (lab.workspace / 'count').exists())
        self.assertTrue(any(r['kind'] == 'tool_started' for r in lab.records()))
        self.assertFalse(any(r['kind'] == 'tool_finished' for r in lab.records()))
        lab.kill(process)
        wait_for(lambda: (lab.workspace / 'child-exited').exists())
        lab.finish(lab.launch(resume=True))
        self.assertEqual((lab.workspace / 'count').read_text(), 'x')
        outputs = {i['call_id']: json.loads(i['output']) for i in lab.requests()[-1]['input'] if i['type'] == 'function_call_output'}
        output = outputs['c1']
        self.assertEqual(outputs['dependent']['execution_state'], 'not_executed')
        self.assertFalse((lab.workspace / 'must-not-execute').exists())
        self.assertEqual(output['execution_state'], 'unknown')
        self.assertFalse(output['success'])
        self.assertEqual(sum(r['kind'] == 'tool_started' for r in lab.records()), 1)

    def test_killed_after_durable_result_does_not_rerun_tool(self):
        lab = self.start([call(), {'text': 'lost answer'}, {'text': 'resampled answer'}], ['--approval', 'auto'], hold_index=1)
        process = lab.launch(prompt='increment once')
        self.assertTrue(lab.received.wait(5))
        self.assertTrue(any(r['kind'] == 'tool_finished' for r in lab.records()))
        lab.kill(process)
        lab.finish(lab.launch(resume=True))
        self.assertEqual((lab.workspace / 'count').read_text(), 'x')
        self.assertEqual(lab.requests()[1]['input'], lab.requests()[2]['input'])
        self.assertEqual(sum(r['kind'] == 'tool_started' for r in lab.records()), 1)
        self.assertTrue(any(e['type'] == 'session.recovered' and e['interrupted_sampling'] for e in lab.events()))

    def test_cancel_then_resume_starts_new_turn_with_paired_history(self):
        lab = self.start([call(), {'text': 'continued'}])
        process = lab.launch(prompt='increment once')
        lab.event('approval.requested')
        process.send_signal(signal.SIGINT)
        lab.finish(process, 130)
        lab.finish(lab.launch(resume=True))
        self.assertFalse((lab.workspace / 'count').exists())
        self.assertEqual(sum(e['type'] == 'turn.started' for e in lab.events()), 2)
        history = lab.requests()[-1]['input']
        self.assertEqual([i['type'] for i in history], ['message', 'function_call', 'function_call_output', 'message'])
        self.assertFalse(json.loads(history[2]['output'])['success'])

    def test_torn_tail_recovers_complete_corruption_rejected(self):
        lab = self.start([{'text': 'done'}])
        lab.finish(lab.launch(prompt='task'))
        with lab.session.open('ab') as f:
            f.write(b'{"version":1,"seq":')
        lab.finish(lab.launch(resume=True))
        self.assertTrue(lab.session.read_bytes().endswith(b'\n'))
        self.assertEqual(len(lab.requests()), 1)
        with lab.session.open('ab') as f:
            f.write(b'invalid complete record\n')
        lab.finish(lab.launch(resume=True), 1)
        self.assertEqual(len(lab.requests()), 1)

    def test_single_writer_and_interrupted_prompt_rejected(self):
        lab = self.start([call(), {'text': 'done'}])
        process = lab.launch(prompt='task')
        approval = lab.event('approval.requested')
        second = lab.launch(resume=True)
        lab.finish(second, 1)
        self.assertIn('locked', second.test_stderr.read_text())
        lab.kill(process)
        before = lab.session.read_bytes()
        second = lab.launch(resume=True, prompt='new task')
        lab.finish(second, 1)
        self.assertIn('without a new prompt', second.test_stderr.read_text())
        self.assertEqual(lab.session.read_bytes(), before)
        self.assertFalse((lab.workspace / 'count').exists())

    def test_capability_changes_and_session_in_workspace_rejected(self):
        lab = self.start([{'text': 'done'}], ['--approval', 'auto'])
        lab.finish(lab.launch(prompt='task'))
        before = lab.session.read_bytes()
        for extra in [['--network', 'allow'], ['--allow-command', 'python3'], ['--filesystem', 'read-only']]:
            process = lab.launch(resume=True, extra=extra)
            lab.finish(process, 1)
            self.assertIn('capability mismatch', process.test_stderr.read_text())
        self.assertEqual(lab.session.read_bytes(), before)
        self.assertEqual(len(lab.requests()), 1)
        lab.session = lab.workspace / 'forbidden.jsonl'
        process = lab.launch(prompt='task')
        lab.finish(process, 1)
        self.assertIn('session rollout must be outside', process.test_stderr.read_text())
        self.assertFalse(lab.session.exists())

    def test_compaction_bounded_requests_full_rollout_and_resume(self):
        responses = [call(f'read-{i}', name='read_file', arguments={'path': 'large.txt'}) for i in range(10)] + [{'text': 'lost final'}, {'text': 'done'}, {'text': 'followup'}]
        lab = self.start(responses, ['--context-tokens', '10000', '--max-output-tokens', '512', '--tool-output-bytes', '2048'], hold_index=10)
        (lab.workspace / 'large.txt').write_text('内容🙂' * 5000)
        process = lab.launch(prompt='Never modify large.txt. Read and report.')
        self.assertTrue(lab.received.wait(5))
        lab.kill(process)
        lab.finish(lab.launch(resume=True))
        self.assertEqual(lab.requests()[10]['input'], lab.requests()[11]['input'])
        records = lab.records()
        self.assertTrue(any(r['kind'] == 'context_compacted' for r in records))
        full = [r for r in records if r['kind'] == 'tool_finished']
        self.assertEqual(len(full), 10)
        self.assertTrue(all(len(r['data']['result']['output'].encode()) > 20000 for r in full))
        self.assertTrue(all(len(r['data']['model_output'].encode()) <= 2048 for r in full))
        for request in lab.requests():
            estimate = len(json.dumps(request['input'], ensure_ascii=False, separators=(',', ':')).encode()) + len(json.dumps(request['tools'], ensure_ascii=False, separators=(',', ':')).encode()) + 1024 + 512
            self.assertLessEqual(estimate, 10000)
            self.assertEqual(request['input'][0]['content'][0]['text'], 'Never modify large.txt. Read and report.')
            pending = set()
            for item in request['input']:
                if item['type'] == 'function_call':
                    pending.add(item['call_id'])
                if item['type'] == 'function_call_output':
                    self.assertIn(item['call_id'], pending)
                    pending.remove(item['call_id'])
            self.assertFalse(pending)
        lab.finish(lab.launch(resume=True, prompt='What did you find?'))
        self.assertEqual(len(lab.requests()), 13)
        self.assertTrue(any(i.get('role') == 'user' and i['content'][0]['text'] == 'Never modify large.txt. Read and report.' for i in lab.requests()[-1]['input']))
        self.assertTrue(any(e['type'] == 'context.compacted' for e in lab.events()))

    def test_oversized_pinned_prompt_fails_before_model_request(self):
        lab = self.start([], ['--context-tokens', '5000', '--max-output-tokens', '512'])
        process = lab.launch(prompt='x' * 10000)
        self.assertEqual(lab.finish(process, 1)[-1]['type'], 'turn.failed')
        self.assertIn('context budget exceeded', process.test_stderr.read_text())
        self.assertEqual(lab.requests(), [])

    def test_crashed_sampling_budget_survives_resume(self):
        lab = self.start([{'text': 'lost'}], ['--max-steps', '1'], hold_index=0)
        process = lab.launch(prompt='task')
        self.assertTrue(lab.received.wait(5))
        lab.kill(process)
        lab.options = []  # Resume defaults to the saved budget, not a fresh 16 steps.
        process = lab.launch(resume=True)
        lab.finish(process, 1)
        self.assertIn('max_steps=1', process.test_stderr.read_text())
        self.assertEqual(len(lab.requests()), 1)


if __name__ == '__main__':
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    unittest.main(verbosity=2)
