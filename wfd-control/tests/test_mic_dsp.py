"""The firmware's mic processing (mic_dsp.h), built on the PC.

Two kinds of input:
- simulated ADC signals (tests/host/mic_dump.cpp): hum + hiss with the bottom cut off by the
  ADC dead zone, and a tone added for frames 60-99. Rows: colBass colTreble level% band0..6%.
- a real recording from the clock (tests/host/mictrace_room.txt, MICTRACE at 31 frames/s):
  quiet, busy (typing/talking), loud (music and claps), quiet again. Replayed through the same
  loudness code. Rows: column level%.
Needs a C++ compiler; skipped if there is none.
"""
from pathlib import Path

import pytest

from host import hostbuild

HOST = Path(__file__).parent / "host"
BG, TONE, AFTER = slice(20, 60), slice(62, 100), slice(115, 140)


@pytest.fixture(scope="module")
def exe(tmp_path_factory):
    return hostbuild.build(HOST / "mic_dump.cpp", tmp_path_factory.mktemp("host"))


@pytest.fixture(scope="module")
def mic(exe):
    return lambda hz, amp: hostbuild.run(exe, hz, amp)


def avg(rows, col):
    return sum(r[col] for r in rows) / len(rows)


def test_dead_silence_shows_nothing(mic):
    assert all(v == 0 for row in mic(0, 0) for v in row)


def test_room_hum_and_hiss_stay_dark(mic):
    rows = mic(0, 1)
    assert all(v == 0 for r in rows[BG] for v in r), "nothing should move in a quiet room"


@pytest.mark.parametrize("hz, near, far, band", [(200, 0, 1, (1, 2)), (2500, 1, 0, (5,))])
def test_loud_tone_lights_its_own_column_and_band(mic, hz, near, far, band):
    tone = mic(hz, 800)[TONE]
    assert avg(tone, near) >= 8
    assert avg(tone, far) <= 2
    assert max(range(7), key=lambda b: avg(tone, 3 + b)) in band


@pytest.mark.parametrize("hz, near, far", [(200, 0, 1), (2500, 1, 0)])
def test_quiet_tone_shows_smaller_on_the_right_side(mic, hz, near, far):
    quiet, loud = mic(hz, 150)[TONE], mic(hz, 800)[TONE]
    assert 1 <= avg(quiet, near) < avg(loud, near)
    assert avg(quiet, near) > avg(quiet, far)


@pytest.mark.parametrize("hz", [200, 2500])
def test_display_falls_back_after_the_sound(mic, hz):
    rows = mic(hz, 800)
    assert all(v == 0 for r in rows[AFTER] for v in r)


def test_columns_fall_smoothly_not_instantly(mic):
    tail = [r[0] for r in mic(200, 800)[100:118]]
    assert tail[0] > 6 and tail[-1] == 0
    assert all(b <= a for a, b in zip(tail, tail[1:])), "falls without bouncing back up"
    assert all(a - b <= 1 for a, b in zip(tail, tail[1:])), "no sudden drops"


# ---- the real room ----
def segment(rows, a, b, fps=31):
    return [r[0] for r in rows[int(a * fps):int(b * fps)]]


@pytest.fixture(scope="module")
def room(exe):
    rows = hostbuild.run(exe, "replay", HOST / "mictrace_room.txt")
    assert len(rows) > 1000
    return rows


def test_recorded_quiet_stays_dark(room):
    for a, b in [(1, 4), (34, 39)]:
        col = segment(room, a, b)
        assert sum(1 for v in col if v) / len(col) <= 0.06, f"quiet {a}-{b}s lights up"
        assert max(col) <= 4


def test_recorded_music_and_claps_fill_the_column(room):
    col = segment(room, 20, 33)
    assert sum(1 for v in col if v) / len(col) >= 0.7
    assert max(col) == 12
    assert sum(col) / len(col) >= 4
