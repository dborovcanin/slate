#!/usr/bin/env python3
"""Slate v1 selection transform: :run uppercase (with a visual selection)."""
import json
import sys

request = json.load(sys.stdin)
if request["version"] != 1:
    raise ValueError("unsupported Slate script protocol")
json.dump({"text": request["text"].upper()}, sys.stdout, ensure_ascii=False)
