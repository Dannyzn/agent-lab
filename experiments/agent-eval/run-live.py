#!/usr/bin/env python3
"""Provider-neutral live coding eval. Connection profiles and traces stay local."""
import argparse
import getpass
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

LAB = Path(__file__).resolve().parents[2]
TASKS = LAB / 'contracts/coding-tasks'


def snapshot(workspace):
    return {str(path.relative_to(workspace)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in workspace.rglob('*') if path.is_file() and '.git' not in path.relative_to(workspace).parts}


def events_from(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().split('\n')[:-1] if line.strip()]


def approval_allowed(request, task, workspace):
    capabilities = request.get('capabilities', {})
    sandbox = capabilities.get('sandbox', {})
    if (Path(capabilities.get('workspace', '/')) != workspace.resolve() or
            sandbox.get('enabled') is not True or sandbox.get('network') != 'deny'):
        return False
    arguments = request.get('arguments', {})
    if request.get('tool') == 'apply_patch':
        patch = arguments.get('patch', '')
        sections = re.findall(r'^\*\*\* (?:Update|Add|Delete) File: (.+)$', patch, re.M)
        return bool(sections) and all(name in task['editable_files'] for name in sections) and not any(marker in patch for marker in ['*** Add File:', '*** Delete File:', '*** Move to:'])
    if request.get('tool') != 'exec_command':
        return False
    argv = arguments.get('argv', [])
    if not argv or not all(isinstance(item, str) for item in argv):
        return False
    if argv[0] == 'python3':
        options = argv[1:]
        if options and options[0] == '-B':
            options = options[1:]
        # Only test execution is approved; arbitrary -c/scripts cannot bypass
        # patch-level protection of evaluation files.
        return len(options) >= 2 and options[:2] == ['-m', 'unittest']
    if argv[0] == 'git':
        return len(argv) >= 2 and argv[1] in ['diff', 'status']
    return argv[0] in ['ls', 'cat', 'rg']


def evaluate(workspace, argv):
    # Independent evaluator is host-controlled and runs read-only. No API key
    # or inherited personal environment is passed to the candidate code.
    source = (LAB / 'rust/agent-sandbox/src/lib.rs').read_text()
    profile = source.split('let mut profile = r#"', 1)[1].split('"#', 1)[0]
    executable = '/Library/Developer/CommandLineTools/usr/bin/python3'
    with tempfile.TemporaryDirectory(prefix='agent-eval-') as scratch:
        result = subprocess.run(['/usr/bin/sandbox-exec', '-p', profile,
                                 '-D', 'WORKSPACE=' + str(workspace.resolve()),
                                 '-D', 'SCRATCH=' + str(Path(scratch).resolve()), executable] + argv[1:],
                                cwd=str(workspace), env={'PATH': '/usr/bin:/bin', 'HOME': scratch, 'TMPDIR': scratch, 'PYTHONDONTWRITEBYTECODE': '1'},
                                capture_output=True, text=True, timeout=20)
    return result


def run_case(task_id, config, key, root, token_limit, max_steps, deadline):
    task_dir = TASKS / task_id
    task = json.loads((task_dir / 'task.json').read_text())
    case = root / task_id
    workspace = case / 'workspace'
    shutil.copytree(str(task_dir / 'initial'), str(workspace))
    subprocess.run(['git', '-C', str(workspace), 'init', '-q'], check=True)
    subprocess.run(['git', '-C', str(workspace), 'add', '.'], check=True)
    before = snapshot(workspace)
    before_tests = evaluate(workspace, task['test_argv'])
    (case / 'test-before.txt').write_text(before_tests.stdout + before_tests.stderr)
    assert (before_tests.returncode == 0) == task['initial_tests_should_pass'], 'invalid baseline fixture'
    event_path = case / 'events.jsonl'
    command = [str(LAB / 'target/debug/our-agent'), '--workspace', str(workspace), '--base-url', config['base_url'],
               '--model', config['model'], '--reasoning-effort', config['reasoning_effort'], '--max-output-tokens', str(token_limit),
               '--max-steps', str(max_steps), '--json', '--events', str(event_path), '--approval', 'ask',
               '--filesystem', 'workspace-write', '--network', 'deny', task['prompt'] + '\nFor this acceptance run, command approvals allow only python3 -m unittest (with optional -B), git diff/status, ls, cat, and rg. Use file tools for inspection and apply_patch for all edits; arbitrary Python -c/script commands are not approved.']
    environment = dict(os.environ, OPENAI_API_KEY=key)
    environment.pop(config['api_key_env'], None)
    approvals = []
    handled = set()
    started = time.monotonic()
    timed_out = False
    with (case / 'stdout.jsonl').open('w') as stdout, (case / 'stderr.txt').open('w') as stderr:
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr, env=environment, text=True)
        try:
            while process.poll() is None:
                for event in events_from(event_path):
                    if event['type'] != 'approval.requested' or event['approval_id'] in handled:
                        continue
                    handled.add(event['approval_id'])
                    allow = approval_allowed(event, task, workspace)
                    approvals.append({'approval_id': event['approval_id'], 'tool': event['tool'], 'allowed': allow})
                    process.stdin.write(json.dumps({'approval_id': event['approval_id'], 'decision': 'allow' if allow else 'deny'}) + '\n')
                    process.stdin.flush()
                if time.monotonic() - started > deadline:
                    timed_out = True
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                    break
                time.sleep(0.04)
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            process.stdin.close()
    elapsed = time.monotonic() - started
    events = events_from(event_path)
    items = [e['item'] for e in events if e['type'] == 'item.completed' and e['item']['type'] != 'agent_message']
    after_tests = evaluate(workspace, task['test_argv'])
    oracle = evaluate(workspace, ['python3', '-B', '-c', (task_dir / 'oracle.py').read_text()])
    (case / 'test-after.txt').write_text(after_tests.stdout + after_tests.stderr)
    (case / 'independent-oracle.txt').write_text(oracle.stdout + oracle.stderr)
    after = snapshot(workspace)
    changed = sorted(path for path in set(before) | set(after) if before.get(path) != after.get(path))
    unchanged_protected = all(after.get(path) == value for path, value in before.items() if path not in task['editable_files'])
    scoped = set(changed).issubset(task['editable_files'])
    required = set(task['required_changed_files']).issubset(changed)
    diff = subprocess.run(['git', '-C', str(workspace), 'diff', '--no-ext-diff', '--no-textconv'], capture_output=True, text=True, check=True).stdout
    (case / 'fixture.diff').write_text(diff)
    result = {'task': task_id, 'exit_code': process.returncode, 'timed_out': timed_out,
              'elapsed_seconds': round(elapsed, 2), 'initial_tests_passed': before_tests.returncode == 0,
              'visible_tests_passed': after_tests.returncode == 0, 'independent_oracle_passed': oracle.returncode == 0,
              'protected_files_unchanged': unchanged_protected, 'changed_files': changed,
              'edit_scope_respected': scoped, 'required_files_changed': required,
              'event_count': len(events), 'tool_calls': len(items),
              'tool_failures': sum(not item['result']['success'] for item in items),
              'approvals': len(approvals), 'denied_approvals': sum(not item['allowed'] for item in approvals),
              'terminal_event': events[-1]['type'] if events else None}
    result['functional_pass'] = process.returncode == 0 and after_tests.returncode == 0 and oracle.returncode == 0 and unchanged_protected and scoped and required and result['terminal_event'] == 'turn.completed'
    result['strict_pass'] = result['functional_pass'] and result['denied_approvals'] == 0
    (case / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    (case / 'approval-decisions.json').write_text(json.dumps(approvals, indent=2) + '\n')
    print(json.dumps(result), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--profile', type=Path, required=True)
    parser.add_argument('--task', choices=['fix-calculator', 'invoice-discount', 'both'], default='both')
    parser.add_argument('--max-output-tokens', type=int, default=4096)
    parser.add_argument('--max-steps', type=int, default=16)
    parser.add_argument('--deadline-seconds', type=int, default=240)
    args = parser.parse_args()
    config = json.loads(args.profile.read_text())
    key = os.environ.get(config['api_key_env']) or getpass.getpass('Evaluation API key (not saved): ')
    if not key:
        raise SystemExit('No key supplied')
    artifacts = LAB / 'experiments/agent-eval/artifacts'
    artifacts.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='run-', dir=str(artifacts)))
    # Provider identity is private and stays only in ignored local artifacts.
    (root / 'provider.json').write_text(json.dumps({k: config[k] for k in ['base_url', 'model', 'reasoning_effort']}, indent=2) + '\n')
    tasks = ['fix-calculator', 'invoice-discount'] if args.task == 'both' else [args.task]
    results = []
    for task in tasks:
        print('Starting independent task: ' + task, flush=True)
        results.append(run_case(task, config, key, root, args.max_output_tokens, args.max_steps, args.deadline_seconds))
    summary = {'runtime_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=str(LAB), text=True).strip(),
               'max_output_tokens': args.max_output_tokens, 'max_steps': args.max_steps,
               'results': results, 'all_functional_passed': all(r['functional_pass'] for r in results),
               'all_strict_passed': all(r['strict_pass'] for r in results)}
    (root / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    for path in root.rglob('*'):
        if path.is_file() and key.encode() in path.read_bytes():
            raise RuntimeError('Credential detected in local experiment artifact')
    print('Private artifacts: ' + str(root), flush=True)
    return 0 if summary['all_functional_passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
