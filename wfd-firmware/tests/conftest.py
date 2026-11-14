"""The firmware's host tests: its C++ engines built on the PC (host/), its tools, and opt-in
checks against a real clock (test_device_*.py, `-m device`).

oracle/ holds frozen copies of a few modules from the retired Python app (legacy/python-app/)
that these tests compare the firmware against. They are test oracles only, not app code.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "oracle"))
