#!/usr/bin/env python3
"""Bounded live acceptance for durable coding sessions; provider material stays local."""
import argparse
import getpass
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

LAB = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('coding_eval', LAB / 'experiments/agent-eval/run-live.py')
eval_lib = importlib.util.module_from_spec(spec)
spec.loader.exec_module(eval_lib)

GUARD = ('\nRun the visible unittest suite before and after edits. Do not change tests or policy documents. '
         'Only existing UTF-8 implementation files may be updated. Use read_file/search for inspection '
         'and apply_patch for edits. Commands approved in this acceptance run are python3 -B -m unittest '
         '(or without -B), git diff/status, ls, cat, and rg; arbitrary Python -c/scripts are not approved. '
         'Before finishing, execute the unittest suite after your changes. '
         'Patches use *** Begin Patch, *** Update File: path, @@, context/-old/+new lines, *** End Patch.')


def records(path):
    return [row['record'] for row in eval_lib.events_from(path)]


def bounded_run(case, task, config, key, options, prompt=None, resume=False, inject=None, deadline=360):
    workspace = case / 'workspace'
    event_path = case / 'events.jsonl'
    session_path = case / 'session.jsonl'
    index = len(list(case.glob('stderr-*.txt')))
    initial_events = len(eval_lib.events_from(event_path))
    argv = [str(LAB / 'target/debug/our-agent'), '--workspace', str(workspace),
            '--base-url', config['base_url'], '--model', config['model'],
            '--max-output-tokens', '4096', '--max-steps', '32', '--approval', 'ask',
            '--json', '--events', str(event_path), '--network', 'deny',
            '--resume' if resume else '--session', str(session_path)] + options
    if config.get('reasoning_effort'):
        argv += ['--reasoning-effort', config['reasoning_effort']]
    if prompt is not None:
        argv.append(prompt + GUARD)
    environment = dict(os.environ)
    environment.pop(config.get('api_key_env', ''), None)
    environment['OPENAI_API_KEY'] = key
    handled = set()
    decisions = []
    started = time.monotonic()
    injected = False
    timed_out = False
    crash_state = None
    with (case / f'stdout-{index}.jsonl').open('w') as stdout, (case / f'stderr-{index}.txt').open('w') as stderr:
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr,
                                   text=True, env=environment)
        try:
            while process.poll() is None:
                current = eval_lib.events_from(event_path)[initial_events:]
                patched = any(e['type'] == 'item.completed' and e['item'].get('name') == 'apply_patch'
                              and e['item']['result']['success'] for e in current)
                compacted = any(e['type'] == 'context.compacted' for e in current)
                for event in current:
                    if event['type'] != 'approval.requested' or event['approval_id'] in handled:
                        continue
                    handled.add(event['approval_id'])
                    if (inject == 'after-edit' and patched) or (inject == 'after-compaction' and compacted):
                        # The CLI is blocked awaiting an approval we have not sent.
                        # There is no running approved command at this injection point.
                        crash_state = {'approval_id': event['approval_id'], 'call_id': event['call_id'],
                                       'tool': event['tool'], 'edited_before_crash': patched,
                                       'compacted_before_crash': compacted,
                                       'workspace_snapshot': eval_lib.snapshot(workspace),
                                       'turn_id': event['turn_id'], 'thread_id': event['thread_id']}
                        process.kill()
                        process.wait(timeout=5)
                        injected = True
                        break
                    allow = eval_lib.approval_allowed(event, task, workspace)
                    decisions.append({'approval_id': event['approval_id'], 'tool': event['tool'], 'allowed': allow})
                    process.stdin.write(json.dumps({'approval_id': event['approval_id'], 'decision': 'allow' if allow else 'deny'}) + '\n')
                    process.stdin.flush()
                if injected:
                    break
                if time.monotonic() - started > deadline:
                    timed_out = True
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                    break
                time.sleep(.025)
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            process.stdin.close()
    result = {'process_index': index, 'exit_code': process.returncode,
              'elapsed_seconds': round(time.monotonic() - started, 2),
              'timed_out': timed_out, 'injected': injected, 'crash_state': crash_state,
              'approvals': decisions}
    (case / f'process-{index}.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def prepare(case, initial):
    workspace = case / 'workspace'
    shutil.copytree(initial, workspace)
    subprocess.run(['git', '-C', str(workspace), 'init', '-q'], check=True)
    subprocess.run(['git', '-C', str(workspace), 'add', '.'], check=True)
    return workspace


def check_case(case, task, before, oracle_source, label):
    workspace = case / 'workspace'
    tests = eval_lib.evaluate(workspace, task['test_argv'])
    oracle = eval_lib.evaluate(workspace, ['python3', '-B', '-c', oracle_source])
    (case / f'{label}-tests.txt').write_text(tests.stdout + tests.stderr)
    (case / f'{label}-oracle.txt').write_text(oracle.stdout + oracle.stderr)
    after = eval_lib.snapshot(workspace)
    changed = sorted(p for p in before.keys() | after.keys() if before.get(p) != after.get(p))
    scoped = set(changed).issubset(task['editable_files'])
    protected = all(after.get(p) == value for p, value in before.items() if p not in task['editable_files'])
    required = set(task['required_changed_files']).issubset(changed)
    diff = subprocess.run(['git', '-C', str(workspace), 'diff', '--no-ext-diff', '--no-textconv'],
                          capture_output=True, text=True, check=True)
    (case / f'{label}.diff').write_text(diff.stdout)
    return {'visible_tests_passed': tests.returncode == 0, 'oracle_passed': oracle.returncode == 0,
            'protected_files_unchanged': protected, 'changed_files': changed,
            'edit_scope_respected': scoped, 'required_files_changed': required}


def lifecycle(case):
    journal = records(case / 'session.jsonl')
    events = [r['data']['event'] for r in journal if r['kind'] == 'event']
    history = []
    for row in journal:
        kind = row['kind']
        if kind == 'turn_started':
            history.append({'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': row['data']['prompt']}]})
        elif kind == 'model_returned' and row['data']['response'].get('text') is not None:
            history.append({'type': 'message', 'role': 'assistant', 'content': [{'type': 'output_text', 'text': row['data']['response']['text']}]})
        elif kind == 'tool_finished':
            history.append({'type': 'function_call_output', 'call_id': row['data']['call_id']})
    calls = [c['call_id'] for r in journal if r['kind'] == 'model_returned' for c in r['data']['response']['tool_calls']]
    outputs = [r['data']['call_id'] for r in journal if r['kind'] == 'tool_finished']
    ids = [e['event_id'] for e in events]
    completed = [e for e in events if e['type'] == 'item.completed' and e['item']['type'] != 'agent_message']
    return {'thread_count': len({e['thread_id'] for e in events}),
            'turn_count': sum(e['type'] == 'turn.started' for e in events),
            'completed_turns': sum(e['type'] == 'turn.completed' for e in events),
            'model_steps': sum(r['kind'] == 'sampling_started' for r in journal),
            'tool_calls': len(completed), 'tool_failures': sum(not e['item']['result']['success'] for e in completed),
            'compactions': sum(r['kind'] == 'context_compacted' for r in journal),
            'compaction_estimates': [{k: r['data'][k] for k in ['before_tokens', 'after_tokens', 'summarized_pairs']}
                                     for r in journal if r['kind'] == 'context_compacted'],
            'call_result_pairs_valid': calls == outputs and len(calls) == len(set(calls)),
            'event_ids_unique': len(ids) == len(set(ids)),
            'canonical_user_messages': sum(i.get('role') == 'user' for i in history),
            'canonical_assistant_messages': sum(i.get('role') == 'assistant' for i in history)}


def summarize(case, processes, checks, extra=None):
    result = {'processes': [{k: v for k, v in p.items() if k not in ['approvals', 'crash_state']} for p in processes],
              **checks, **lifecycle(case), **(extra or {})}
    result['denied_approvals'] = sum(not d['allowed'] for p in processes for d in p['approvals'])
    result['functional_pass'] = processes[-1]['exit_code'] == 0 and not any(p['timed_out'] for p in processes) and all(checks[k] for k in [
        'visible_tests_passed', 'oracle_passed', 'protected_files_unchanged', 'edit_scope_respected', 'required_files_changed'])
    result['strict_pass'] = result['functional_pass'] and not result['denied_approvals'] and result['call_result_pairs_valid'] and result['event_ids_unique']
    return result


def repair_resume(root, config, key, deadline):
    case = root / 'repair-resume'
    task_dir = LAB / 'contracts/coding-tasks/invoice-discount'
    task = json.loads((task_dir / 'task.json').read_text())
    workspace = prepare(case, task_dir / 'initial')
    before = eval_lib.snapshot(workspace)
    baseline = eval_lib.evaluate(workspace, task['test_argv'])
    (case / 'baseline-tests.txt').write_text(baseline.stdout + baseline.stderr)
    assert baseline.returncode != 0, 'baseline must expose missing feature'
    first = bounded_run(case, task, config, key, [], prompt=task['prompt'], inject='after-edit', deadline=deadline)
    processes = [first]
    if first['injected']:
        processes.append(bounded_run(case, task, config, key, [], resume=True, deadline=deadline))
    checks = check_case(case, task, before, (task_dir / 'oracle.py').read_text(), 'repair')
    result = summarize(case, processes, checks, {'interruption_reached': first['injected']})
    events = eval_lib.events_from(case / 'events.jsonl')
    crash = first['crash_state']
    fresh = [e for e in events if e['type'] == 'approval.requested' and crash and e['call_id'] == crash['call_id']]
    result['approval_renewed'] = len(fresh) == 2 and fresh[0]['approval_id'] != fresh[1]['approval_id']
    result['strict_pass'] = result['strict_pass'] and first['injected'] and result['approval_renewed'] and result['turn_count'] == 1
    (case / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'case': 'repair-resume', **result}), flush=True)
    return result, case, task


FOLLOWUP = '''Extend the invoice implementation from the previous task. calculate_total and create_quote must accept a one-shot iterable of prices (including generators), not just lists. Materialize it only once when needed. Reject any negative price or any non-integer price (including bool) with ValueError, without mutating the caller's list. Keep the complete discount behavior and exact quote dictionary shape from the original task. Only invoice/pricing.py and invoice/service.py may change for this turn; leave CLI, tests, and other files unchanged. Run all visible tests and inspect the prior work before editing.'''
FOLLOWUP_ORACLE = '''
from invoice.pricing import calculate_total
from invoice.service import create_quote
for values, percent in [([], 0), ([1, 2], 33), ([123, 456, 789], 17), ([10**12, 7], 3), ([0, 999], 100)]:
    total = sum(values) * (100 - percent) // 100
    assert calculate_total((x for x in values), percent) == total
    assert create_quote((x for x in values), percent) == {'subtotal_cents':sum(values), 'discount_cents':sum(values)-total, 'total_cents':total}
for bad in [[-1], [True], [1.5], ['1'], [None], [1, -2], [0, False]]:
    for function in [calculate_total, create_quote]:
        try:
            function((x for x in bad), 0)
        except ValueError:
            pass
        else:
            raise AssertionError('invalid price accepted')
print('independent one-shot iterable and price-validation checks passed')
'''


def followup(case, config, key, deadline):
    task_dir = LAB / 'contracts/coding-tasks/invoice-discount'
    task = json.loads((task_dir / 'task.json').read_text())
    task['editable_files'] = ['invoice/pricing.py', 'invoice/service.py']
    task['required_changed_files'] = list(task['editable_files'])
    before = eval_lib.snapshot(case / 'workspace')
    process = bounded_run(case, task, config, key, [], prompt=FOLLOWUP, resume=True, deadline=deadline)
    checks = check_case(case, task, before, (task_dir / 'oracle.py').read_text() + FOLLOWUP_ORACLE, 'followup')
    result = summarize(case, [process], checks)
    result['strict_pass'] = result['strict_pass'] and result['thread_count'] == 1 and result['completed_turns'] == 2
    (case / 'followup-result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'case': 'same-thread-followup', **result}), flush=True)
    return result


def policy_reads_complete(case, policy_files):
    journal = records(case / 'session.jsonl')
    calls = {c['call_id']: c for r in journal if r['kind'] == 'model_returned'
             for c in r['data']['response']['tool_calls']}
    successful = set()
    for row in journal:
        if row['kind'] != 'tool_finished':
            continue
        call = calls.get(row['data']['call_id'], {})
        path = call.get('arguments', {}).get('path')
        result = row['data']['result']
        if call.get('name') == 'read_file' and path in policy_files and result.get('success') is True:
            if result.get('output') == (case / 'workspace' / path).read_text():
                successful.add(path)
    return set(policy_files).issubset(successful)


def long_context(root, config, key, deadline, control=False):
    name = 'long-context-control' if control else 'long-context'
    case = root / name
    fixture = LAB / 'contracts/coding-tasks/shipping-policies'
    task = json.loads((fixture / 'task.json').read_text())
    workspace = prepare(case, fixture / 'initial')
    before = eval_lib.snapshot(workspace)
    baseline = eval_lib.evaluate(workspace, task['test_argv'])
    (case / 'baseline-tests.txt').write_text(baseline.stdout + baseline.stderr)
    assert baseline.returncode != 0
    options = ['--context-tokens', '65536' if control else '16000', '--tool-output-bytes', '8192']
    first = bounded_run(case, task, config, key, options, prompt=task['prompt'], inject=None if control else 'after-compaction', deadline=deadline)
    processes = [first]
    if first['injected']:
        processes.append(bounded_run(case, task, config, key, options, resume=True, deadline=deadline))
    checks = check_case(case, task, before, (fixture / 'oracle.py').read_text(), 'shipping')
    result = summarize(case, processes, checks, {'interruption_reached': first['injected'],
                        'all_policy_documents_read': policy_reads_complete(case, task['policy_files'])})
    result['strict_pass'] = result['strict_pass'] and result['all_policy_documents_read'] and (control or (first['injected'] and result['compactions'] > 0))
    result['control_without_interruption'] = control
    (case / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'case': name, **result}), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--profile', type=Path, required=True)
    parser.add_argument('--case', choices=['all', 'repair-resume', 'long-context', 'long-context-control'], default='all')
    parser.add_argument('--deadline-seconds', type=int, default=360)
    args = parser.parse_args()
    config = json.loads(args.profile.read_text())
    key = os.environ.get(config['api_key_env']) or getpass.getpass('Evaluation API key (not saved): ')
    if not key:
        raise SystemExit('No key supplied')
    artifacts = LAB / 'experiments/session-eval/artifacts'
    artifacts.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='run-', dir=artifacts))
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    source_diff = subprocess.check_output(['git', 'diff', 'HEAD', '--', 'rust', 'Cargo.toml', 'Cargo.lock'])
    (root / 'runtime-source.diff').write_bytes(source_diff)
    binary_sha = hashlib.sha256((LAB / 'target/debug/our-agent').read_bytes()).hexdigest()
    (root / 'provider.json').write_text(json.dumps({k: config.get(k) for k in ['base_url', 'model', 'reasoning_effort']}, indent=2) + '\n')
    results = {}
    if args.case in ['all', 'repair-resume']:
        result, case, task = repair_resume(root, config, key, args.deadline_seconds)
        results['repair-resume'] = result
        if result['strict_pass']:
            results['same-thread-followup'] = followup(case, config, key, args.deadline_seconds)
    if args.case in ['all', 'long-context', 'long-context-control']:
        control = args.case == 'long-context-control'
        results['long-context-control' if control else 'long-context'] = long_context(root, config, key, args.deadline_seconds, control=control)
    summary = {'runtime_commit': revision, 'tracked_source_modified': bool(source_diff),
               'source_diff_sha256': hashlib.sha256(source_diff).hexdigest(), 'binary_sha256': binary_sha,
               'results': results, 'all_strict_passed': bool(results) and all(r['strict_pass'] for r in results.values())}
    for path in root.rglob('*'):
        if path.is_file() and '.git' not in path.relative_to(root).parts and key.encode() in path.read_bytes():
            raise RuntimeError('Credential detected in local artifact')
    (root / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print('Private artifacts: ' + str(root), flush=True)
    return 0 if summary['all_strict_passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
