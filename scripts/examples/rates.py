#!/usr/bin/env python3
"""Slate [currency] rates: daily rates from https://open.er-api.com.

Rates by Exchange Rate API (https://www.exchangerate-api.com); see its terms.
An optional argv entry after the script path picks the base currency.
"""
import datetime
import json
import sys
import urllib.request

base = sys.argv[1] if len(sys.argv) > 1 else "EUR"
with urllib.request.urlopen(f"https://open.er-api.com/v6/latest/{base}", timeout=15) as response:
    data = json.load(response)
if data.get("result") != "success":
    raise RuntimeError(f"rates request failed: {data.get('error-type', 'unknown error')}")
as_of = datetime.datetime.fromtimestamp(data["time_last_update_unix"], datetime.timezone.utc)
json.dump({"base": data["base_code"], "rates": data["rates"], "as_of": as_of.date().isoformat()}, sys.stdout)
