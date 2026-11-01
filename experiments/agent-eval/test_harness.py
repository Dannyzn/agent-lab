import importlib.util
from pathlib import Path
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('eval_runner', HERE / 'run-live.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ApprovalScopeTests(unittest.TestCase):
    def setUp(self):
        self.workspace = HERE.resolve()
        self.task = {'editable_files': ['implementation.py']}

    def request(self, tool, arguments):
        return {'tool': tool, 'arguments': arguments, 'capabilities': {
            'workspace': str(self.workspace), 'sandbox': {'enabled': True, 'network': 'deny'}}}

    def test_test_file_patch_is_denied_even_if_noop(self):
        patch = '*** Begin Patch\n*** Update File: test_implementation.py\n@@\n context\n*** End Patch'
        self.assertFalse(runner.approval_allowed(self.request('apply_patch', {'patch': patch}), self.task, self.workspace))

    def test_implementation_patch_is_allowed(self):
        patch = '*** Begin Patch\n*** Update File: implementation.py\n@@\n-old\n+new\n*** End Patch'
        self.assertTrue(runner.approval_allowed(self.request('apply_patch', {'patch': patch}), self.task, self.workspace))

    def test_arbitrary_code_command_and_wrong_capabilities_are_denied(self):
        request = self.request('exec_command', {'argv': ['python3', '-c', 'print(1)']})
        self.assertFalse(runner.approval_allowed(request, self.task, self.workspace))
        request = self.request('exec_command', {'argv': ['python3', '-B', '-m', 'unittest', 'discover']})
        self.assertTrue(runner.approval_allowed(request, self.task, self.workspace))
        request['capabilities']['sandbox']['network'] = 'allow'
        self.assertFalse(runner.approval_allowed(request, self.task, self.workspace))


if __name__ == '__main__':
    unittest.main()
