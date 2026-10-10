#!/usr/bin/env python3
"""Slate v1 selection transform: :run format-json, optionally with an indent."""
import json
import sys

request = json.load(sys.stdin)
if request["version"] != 1:
    raise ValueError("unsupported Slate script protocol")
try:
    args = request["args"]
    if len(args) > 1:
        raise ValueError("usage: :run format-json [indent]")
    indent = int(args[0]) if args else 2
    if not 1 <= indent <= 8:
        raise ValueError("indent must be between 1 and 8")
    value = json.loads(request["text"])
except ValueError as error:
    # One stderr line becomes Slate's status message; the selection is kept.
    # JSONDecodeError is a ValueError, so invalid JSON lands here too.
    print(f"format-json: {error}", file=sys.stderr)
    sys.exit(1)
text = json.dumps(value, indent=indent, ensure_ascii=False)
json.dump({"text": text}, sys.stdout, ensure_ascii=False)
