"""The firmware's alarms, timer and forecast logic (wfd-firmware/clock/src/extras.h), built on the PC.

Needs a C++ compiler (MSVC via vswhere, g++ or clang++); skipped if there is none.
"""
import subprocess
from pathlib import Path

import pytest

from host import hostbuild

DRIVER = Path(__file__).parent / "host" / "extras_dump.cpp"


@pytest.fixture(scope="module")
def dump(tmp_path_factory):
    exe = hostbuild.build(DRIVER, tmp_path_factory.mktemp("host"))
    return lambda *args: subprocess.run([str(exe), *map(str, args)], capture_output=True, text=True, check=True).stdout.strip()


@pytest.mark.parametrize("secs, text", [
    (0, "  0:00"), (5, "  0:05"), (299, "  4:59"), (600, " 10:00"), (3599, " 59:59"),
    (3600, " 1H00M"), (3725, " 1H02M"), (86399, "23H59M"), (999999, "23H59M"),
])
def test_timer_fits_the_message_line(dump, secs, text):
    assert dump("timer", secs) == f'"{text}"'


@pytest.mark.parametrize("item, args, text", [
    (0, (78, 55, 81, 3, "F"), "HI 78F"),
    (1, (78, 55, 81, 3, "F"), "LO 55F"),
    (2, (78, 55, 81, 3, "F"), "THU 81"),
    (2, (78, 55, 64, 7, "F"), "MON 64"),          # Sunday's tomorrow wraps to Monday
    (0, (104, 80, 99, 0, "F"), "HI 104"),         # 3 digits: the unit doesn't fit
    (1, (-5, -12, 0, 0, "C"), "LO -12"),
    (1, (3, -2, 0, 0, "C"), "LO -2C"),
])
def test_forecast_text(dump, item, args, text):
    assert dump("forecast", item, *args) == f'"{text}"'


@pytest.mark.parametrize("text, parsed", [
    ("06:30", "6 30"), ("6:30", "6 30"), ("23:59", "23 59"), ("00:00", "0 0"),
    ("24:00", "invalid"), ("12:60", "invalid"), ("6:5", "invalid"), ("630", "invalid"),
    ("06:30pm", "invalid"), ("", "invalid"), ("-1:30", "invalid"),
])
def test_alarm_times(dump, text, parsed):
    assert dump("time", text) == parsed


@pytest.mark.parametrize("wday, hh, mm, due", [
    (0, 6, 30, 0),      # Monday 06:30: the weekday alarm
    (4, 6, 30, 0),      # Friday
    (5, 6, 30, -1),     # Saturday: not a weekday
    (2, 7, 0, -1),      # alarm 1 is off
    (5, 9, 15, 2),      # Saturday 09:15
    (6, 9, 15, 2),      # Sunday
    (3, 9, 15, -1),
    (6, 23, 59, 3),
    (0, 6, 31, -1),
])
def test_alarm_due(dump, wday, hh, mm, due):
    assert dump("due", wday, hh, mm) == str(due)


def test_reads_the_forecast_from_open_meteo(dump):
    doc = ('{"current":{"temperature_2m":71.2,"weather_code":2},"daily_units":{"temperature_2m_max":"°F"},'
           '"daily":{"time":["2026-10-22","2026-10-23"],"weather_code":[2,61,3,0],'
           '"temperature_2m_max":[78.4,81.0,69.5,70.1],"temperature_2m_min":[55.2,60.9,48.0,47.3]}}')
    assert dump("json", "temperature_2m_max", 4, doc) == "78.4 81 69.5 70.1"
    assert dump("json", "weather_code", 4, doc) == "2 61 3 0"
    assert dump("json", "temperature_2m_min", 2, doc) == "55.2 60.9"
    assert dump("json", "missing", 4, doc) == ""
    assert dump("json", "weather_code", 4, '{"weather_code":[null,1]}') == ""
