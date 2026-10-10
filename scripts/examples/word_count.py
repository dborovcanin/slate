#!/usr/bin/env python3
"""Slate v1 status-only script: :run word-count (configured with note input)."""
import json
import sys

request = json.load(sys.stdin)
if request["version"] != 1:
    raise ValueError("unsupported Slate script protocol")
text = request["text"]
message = f"{len(text.split())} words, {len(text)} characters, {len(text.splitlines())} lines"
# Even a status-only response needs text. output = "message" leaves notes alone.
json.dump({"text": "", "message": message}, sys.stdout)
