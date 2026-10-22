#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--events", type=Path, required=True)
    parser.add_argument("--stdout", type=Path)
    parser.add_argument("--requests", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    args = parser.parse_args()

    contract = json.loads(args.contract.read_text(encoding="utf-8"))
    expected = contract["expected"]
    events = [json.loads(line) for line in args.events.read_text().splitlines()]
    requests = [json.loads(line) for line in args.requests.read_text().splitlines()]
    if args.stdout is not None:
        stdout_events = [json.loads(line) for line in args.stdout.read_text().splitlines()]
        assert stdout_events == events
    assert [event["type"] for event in events] == expected["event_types"]
    assert [
        event["item"]["type"] for event in events if "item" in event
    ] == expected["item_types"]
    assert events[-2]["item"]["text"] == expected["final_message"]
    assert len(requests) == expected["model_request_count"]
    assert [len(request["body"]["input"]) for request in requests] == expected[
        "history_lengths"
    ]
    assert all(len(request["body"]["tools"]) == 4 for request in requests)
    assert all(request["body"]["stream"] is True for request in requests)
    final_file = (args.workspace / "calculator.py").read_text(encoding="utf-8")
    assert expected["final_file_contains"] in final_file
    print("rust phase-1 contract: ok")
    print("events={0} requests={1}".format(len(events), len(requests)))


if __name__ == "__main__":
    main()
