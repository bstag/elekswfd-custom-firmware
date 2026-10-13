"""The firmware's C++ grid engine (grid_engine.h), built on the PC and checked against grid_anim.py.

Needs a C++ compiler: MSVC (found with vswhere) or g++/clang++ on PATH. Skipped if there is none.
"""
import math
import re
from pathlib import Path

import pytest

import grid_anim
from host import hostbuild

FW_SRC = hostbuild.FW_SRC
DRIVER = Path(__file__).parent / "host" / "grid_dump.cpp"

DETERMINISTIC = ["spin", "bounce", "wave", "pulse", "scan", "heart", "audio", "mouth"]
RANDOM = ["rain", "sparkle", "life"]
FRAMES = 400


def synthetic_bands(i):
    """Same input grid_dump.cpp feeds the C++ engine: moving waves, with silent gaps."""
    return [(math.sin(i * 0.31 + c * 0.9) + 1) / 2 * (0.0 if i % 23 < 4 else (i % 23) / 22.0) for c in range(7)]


@pytest.fixture(scope="module")
def grid_dump(tmp_path_factory):
    return hostbuild.build(DRIVER, tmp_path_factory.mktemp("host"))


def cpp_frames(exe, name, n=FRAMES):
    return hostbuild.run(exe, name, n)


def py_frames(name, n=FRAMES):
    eng = grid_anim.GridEngine()
    return [eng.frame(name, {"bands": synthetic_bands(i)}) for i in range(n)]


@pytest.mark.parametrize("name", DETERMINISTIC)
def test_matches_python_frame_for_frame(grid_dump, name):
    cpp, py = cpp_frames(grid_dump, name), py_frames(name)
    assert len(cpp) == len(py) == FRAMES
    for i, (a, b) in enumerate(zip(cpp, py)):
        assert a == b, f"{name}: frame {i} differs (C++ {a} vs Python {b})"


@pytest.mark.parametrize("name, shapes", [("audio", 10), ("mouth", 4)])
def test_audio_animations_react(grid_dump, name, shapes):
    """audio: many bar patterns; mouth: closed + 3 openings, all of them used."""
    frames = cpp_frames(grid_dump, name)
    assert len({tuple(f) for f in frames}) >= shapes, f"{name} should follow the bands"


@pytest.mark.parametrize("name", RANDOM)
def test_random_animations_stay_on_grid_and_move(grid_dump, name):
    frames = cpp_frames(grid_dump, name)
    assert all(len(f) == 7 and all(0 <= r < 128 for r in f) for f in frames)
    assert len({tuple(f) for f in frames}) > 20, f"{name} barely changes"
    assert sum(any(f) for f in frames) > FRAMES * 0.5, f"{name} is mostly blank"


def test_rain_falls_one_row_per_frame(grid_dump):
    frames = cpp_frames(grid_dump, "rain")
    for prev, cur in zip(frames, frames[1:]):
        assert cur[1:] == prev[:6], "every drop moves down exactly one row"


def test_life_reseeds_instead_of_dying(grid_dump):
    frames = cpp_frames(grid_dump, "life", 2000)
    blank_runs = max(len(m.group()) for m in re.finditer(r"0*", "".join("0" if not any(f) else "1" for f in frames)))
    assert blank_runs <= 3


def test_engine_names_match_python():
    header = (FW_SRC / "grid_engine.h").read_text(encoding="utf-8")
    names = re.search(r"NAMES\[N_ANIM\]\s*=\s*\{([^}]*)\}", header).group(1)
    fw = re.findall(r'"(\w+)"', names)
    assert fw[0] == "custom"
    assert set(fw) <= set(grid_anim.ANIMATIONS)
    assert set(DETERMINISTIC + RANDOM) == set(fw[1:])
