import json
import unittest
from unittest.mock import patch

from codex_mini.providers import OpenAIResponsesClient


class FakeHTTPResponse:
    def __init__(self, body):
        self.body = json.dumps(body).encode("utf-8")

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        return False

    def read(self):
        return self.body


class OpenAIResponsesClientTest(unittest.TestCase):
    @patch("urllib.request.urlopen")
    def test_maps_function_calls_and_sends_canonical_history(self, urlopen):
        urlopen.side_effect = [
            FakeHTTPResponse(
                {
                    "id": "response-1",
                    "output": [
                        {
                            "type": "function_call",
                            "call_id": "call-1",
                            "name": "read_file",
                            "arguments": '{"path":"README.md"}',
                        }
                    ],
                }
            ),
            FakeHTTPResponse(
                {
                    "id": "response-2",
                    "output": [
                        {
                            "type": "message",
                            "content": [{"type": "output_text", "text": "Done."}],
                        }
                    ],
                }
            ),
        ]
        client = OpenAIResponsesClient("test-model", api_key="test-key")

        first = client.complete(
            [{"type": "message", "role": "user", "content": "Read it"}], []
        )
        second = client.complete(
            [
                {"type": "message", "role": "user", "content": "Read it"},
                {
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "read_file",
                    "arguments": '{"path":"README.md"}',
                },
                {
                    "type": "function_call_output",
                    "call_id": "call-1",
                    "output": "contents",
                }
            ],
            [],
        )

        self.assertEqual(first.tool_calls[0].name, "read_file")
        self.assertEqual(first.tool_calls[0].arguments, {"path": "README.md"})
        self.assertEqual(second.text, "Done.")
        first_payload = json.loads(urlopen.call_args_list[0].args[0].data)
        second_payload = json.loads(urlopen.call_args_list[1].args[0].data)
        self.assertNotIn("previous_response_id", first_payload)
        self.assertNotIn("previous_response_id", second_payload)
        self.assertEqual(
            [item["type"] for item in second_payload["input"]],
            ["message", "function_call", "function_call_output"],
        )


if __name__ == "__main__":
    unittest.main()
