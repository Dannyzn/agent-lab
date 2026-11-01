"""Prove the acceptance harness rejects a visible-test shortcut and injects real crashes."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from http.server import ThreadingHTTPServer

HERE = Path(__file__).resolve().parent
LAB = HERE.parents[1]
spec = importlib.util.spec_from_file_location('session_eval', HERE / 'run-live.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
sys.path.insert(0, str(LAB / 'contracts'))
from mock_responses_server import State, handler_for


def tool(ident, name, arguments):
    return {'call_id': ident, 'name': name, 'arguments': arguments}


def patch_file(path, before, after):
    return f'*** Update File: {path}\n@@\n' + ''.join('-' + line + '\n' for line in before.splitlines()) + ''.join('+' + line + '\n' for line in after.splitlines())


REFERENCE_PRICING = '''RATES = {'north': (350, 90, 5000), 'south': (420, 110, 7000), 'east': (300, 80, 4500), 'west': (390, 95, 6500), 'central': (280, 70, 4000), 'islands': (800, 240, 12000)}

def shipping_cost(weight_grams, zone, subtotal_cents=0):
    if type(weight_grams) is not int or weight_grams <= 0 or type(subtotal_cents) is not int or subtotal_cents < 0 or not isinstance(zone, str) or zone not in RATES:
        raise ValueError('invalid input')
    base, step, threshold = RATES[zone]
    if subtotal_cents >= threshold:
        return 0
    blocks = max(0, (weight_grams - 1) // 500)
    return base + blocks * step
'''
REFERENCE_CLI = '''import argparse
import json
from shipping.pricing import shipping_cost

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--zone', required=True)
    parser.add_argument('--weight-grams', required=True, type=int)
    parser.add_argument('--subtotal-cents', default=0, type=int)
    args = parser.parse_args()
    print(json.dumps({'shipping_cents': shipping_cost(args.weight_grams, args.zone, args.subtotal_cents)}))

if __name__ == '__main__':
    main()
'''


class AcceptanceHarnessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='session-eval-contract-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def serve(self, responses):
        state = State({'responses': responses}, self.root / 'requests.jsonl')
        server = ThreadingHTTPServer(('127.0.0.1', 0), handler_for(state))
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return {'base_url': f'http://127.0.0.1:{server.server_port}/v1', 'model': 'contract-model'}, state

    def test_crash_after_edit_requires_new_approval_and_preserves_result(self):
        case = self.root / 'repair'
        fixture = LAB / 'contracts/coding-tasks/fix-calculator'
        task = json.loads((fixture / 'task.json').read_text())
        workspace = runner.prepare(case, fixture / 'initial')
        before = runner.eval_lib.snapshot(workspace)
        patch = '*** Begin Patch\n*** Update File: calculator.py\n@@\n-    return left - right\n+    return left + right\n*** End Patch'
        config, state = self.serve([
            {'tool_calls': [tool('edit', 'apply_patch', {'patch': patch})]},
            {'tool_calls': [tool('verify', 'exec_command', {'argv': task['test_argv']})]},
            {'text': 'done'},
        ])
        first = runner.bounded_run(case, task, config, 'contract-key', [], prompt=task['prompt'], inject='after-edit', deadline=10)
        self.assertTrue(first['injected'])
        self.assertEqual(first['exit_code'], -9)
        second = runner.bounded_run(case, task, config, 'contract-key', [], resume=True, deadline=10)
        checks = runner.check_case(case, task, before, (fixture / 'oracle.py').read_text(), 'contract')
        result = runner.summarize(case, [first, second], checks)
        self.assertTrue(result['strict_pass'])
        self.assertEqual(state.index, 3)
        self.assertEqual(result['turn_count'], 1)
        events = runner.eval_lib.events_from(case / 'events.jsonl')
        approvals = [e for e in events if e['type'] == 'approval.requested' and e['call_id'] == 'verify']
        self.assertEqual(len(approvals), 2)
        self.assertNotEqual(approvals[0]['approval_id'], approvals[1]['approval_id'])

    def test_compaction_crash_fixture_exercises_all_policies_and_independent_oracle(self):
        fixture = LAB / 'contracts/coding-tasks/shipping-policies'
        task = json.loads((fixture / 'task.json').read_text())
        sections = [patch_file('shipping/pricing.py', (fixture / 'initial/shipping/pricing.py').read_text(), REFERENCE_PRICING),
                    patch_file('shipping/cli.py', (fixture / 'initial/shipping/cli.py').read_text(), REFERENCE_CLI)]
        responses = [
            {'tool_calls': [tool(f'read-{i}', 'read_file', {'path': path}) for i, path in enumerate(task['policy_files'])]},
            {'tool_calls': [tool('edit', 'apply_patch', {'patch': '*** Begin Patch\n' + ''.join(sections) + '*** End Patch'})]},
            {'tool_calls': [tool('verify', 'exec_command', {'argv': task['test_argv']})]},
            {'text': 'done'},
        ]
        config, state = self.serve(responses)
        result = runner.long_context(self.root, config, 'contract-key', 15)
        self.assertTrue(result['strict_pass'])
        self.assertTrue(result['interruption_reached'])
        self.assertGreater(result['compactions'], 0)
        self.assertTrue(result['all_policy_documents_read'])
        self.assertEqual(state.index, 4)
        config, control_state = self.serve(responses)
        control = runner.long_context(self.root, config, 'contract-key', 15, control=True)
        self.assertTrue(control['strict_pass'])
        self.assertTrue(control['control_without_interruption'])
        self.assertFalse(control['interruption_reached'])
        self.assertEqual(control['compactions'], 0)
        self.assertEqual(control_state.index, 4)

    def test_oracle_rejects_shortcut_that_passes_all_visible_tests(self):
        fixture = LAB / 'contracts/coding-tasks/shipping-policies'
        case = self.root / 'mutant'
        task = json.loads((fixture / 'task.json').read_text())
        workspace = runner.prepare(case, fixture / 'initial')
        before = runner.eval_lib.snapshot(workspace)
        (workspace / 'shipping/pricing.py').write_text('''def shipping_cost(weight_grams, zone, subtotal_cents=0):
    if weight_grams <= 0:
        raise ValueError('weight')
    return 0 if subtotal_cents >= 5000 else 350
''')
        (workspace / 'shipping/cli.py').write_text(REFERENCE_CLI)
        result = runner.check_case(case, task, before, (fixture / 'oracle.py').read_text(), 'mutant')
        self.assertTrue(result['visible_tests_passed'])
        self.assertFalse(result['oracle_passed'])
        self.assertTrue(result['protected_files_unchanged'])
        self.assertFalse((workspace / 'oracle.py').exists())

    def test_declaring_a_policy_read_without_a_successful_full_result_is_insufficient(self):
        case = self.root / 'missing-read'
        (case / 'workspace/policies').mkdir(parents=True)
        (case / 'workspace/policies/north.md').write_text('full authoritative policy')
        rows = [{'kind':'model_returned','data':{'response':{'tool_calls':[tool('c','read_file',{'path':'policies/north.md'})]}}},
                {'kind':'tool_finished','data':{'call_id':'c','result':{'success':True,'output':'truncated'}}}]
        (case / 'session.jsonl').write_text(''.join(json.dumps({'record': r}) + '\n' for r in rows))
        self.assertFalse(runner.policy_reads_complete(case, ['policies/north.md']))
        rows[-1]['data']['result']['output'] = 'full authoritative policy'
        (case / 'session.jsonl').write_text(''.join(json.dumps({'record': r}) + '\n' for r in rows))
        self.assertTrue(runner.policy_reads_complete(case, ['policies/north.md']))

    def test_pairing_rejects_duplicate_tool_results(self):
        case = self.root / 'bad-pairs'
        case.mkdir()
        rows = [{'kind': 'model_returned', 'data': {'response': {'tool_calls': [tool('c', 'read_file', {'path':'x'})]}}},
                {'kind': 'tool_finished', 'data': {'call_id': 'c'}},
                {'kind': 'tool_finished', 'data': {'call_id': 'c'}}]
        (case / 'session.jsonl').write_text(''.join(json.dumps({'record': r}) + '\n' for r in rows))
        self.assertFalse(runner.lifecycle(case)['call_result_pairs_valid'])


if __name__ == '__main__':
    unittest.main(verbosity=2)
