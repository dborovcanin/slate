#!/usr/bin/env python3
"""Slate v1 selection transform: :run format-json, optionally with an indent."""
import json
import sys

request = json.load(sys.stdin)
if request["version"] != 1:
    raise ValueError("unsupported Slate script protocol")
args = request["args"]
if len(args) > 1:
    raise ValueError("usage: :run format-json [indent]")
indent = int(args[0]) if args else 2
if not 1 <= indent <= 8:
    raise ValueError("indent must be between 1 and 8")

# Invalid JSON exits nonzero; Slate reports the error and keeps the selection.
value = json.loads(request["text"])
text = json.dumps(value, indent=indent, ensure_ascii=False)
json.dump({"text": text}, sys.stdout, ensure_ascii=False)
