#!/usr/bin/env python3
"""Slate v1 template: :run meeting 'Project X'."""
import datetime
import json
import sys

request = json.load(sys.stdin)
if request["version"] != 1:
    raise ValueError("unsupported Slate script protocol")
title = " ".join(request["args"]) or "Meeting"
text = f"# {title}\n{datetime.date.today().isoformat()}\n\n## Notes\n\n## Actions\n- [ ] \n"
json.dump({"text": text}, sys.stdout)
