#!/usr/bin/env python3
"""Offline [currency] example with invented rates for learning the protocol."""
import json
import sys

# No network or dependencies. Change these demo values to your own rates.
# One EUR buys 1.25 USD or 117 RSD; the base itself has an implicit rate of 1.
# Do not use these invented values as current exchange rates.
json.dump({"base": "EUR", "rates": {"USD": 1.25, "RSD": 117.0}}, sys.stdout)
