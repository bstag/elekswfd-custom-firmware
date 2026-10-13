"""The firmware's C++ grid engine (grid_engine.h), built on the PC and checked against grid_anim.py.

Needs a C++ compiler: MSVC (found with vswhere) or g++/clang++ on PATH. Skipped if there is none.
"""
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

import grid_anim

ROOT = Path(__file__).resolve().parents[2]
FW_SRC = ROOT / "wfd-firmware" / "clock" / "src"
DRIVER = Path(__file__).parent / "host" / "grid_dump.cpp"

DETERMINISTIC = ["spin", "bounce", "wave", "pulse", "scan", "heart"]
RANDOM = ["rain", "sparkle", "life"]
FRAMES = 400


def _msvc_vcvars():
    vswhere = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
                   "Microsoft Visual Studio", "Installer", "vswhere.exe")
    if not vswhere.exists():
        return None
    out = subprocess.run([str(vswhere), "-latest", "-products", "*", "-requires",
                          "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
                         capture_output=True, text=True).stdout.strip()
    bat = Path(out, "VC", "Auxiliary", "Build", "vcvars64.bat") if out else None
    return bat if bat and bat.exists() else None


@pytest.fixture(scope="module")
def grid_dump(tmp_path_factory):
    """Path to a host-compiled grid_dump executable."""
    out = tmp_path_factory.mktemp("host") / "grid_dump.exe"
    gxx = shutil.which("g++") or shutil.which("clang++")
    if gxx:
        cmd = [gxx, "-std=c++17", "-O1", "-I", str(FW_SRC), str(DRIVER), "-o", str(out)]
        r = subprocess.run(cmd, capture_output=True, text=True)
    else:
        vcvars = _msvc_vcvars()
        if not vcvars:
            pytest.skip("no C++ compiler (MSVC, g++ or clang++)")
        cl = f'call "{vcvars}" >nul && cl /nologo /std:c++17 /EHsc /O1 /I "{FW_SRC}" "{DRIVER}" /Fe:"{out}"'
        # one raw string: cmd needs these quotes verbatim (a list would get them \"-escaped)
        r = subprocess.run(f'cmd /d /s /c "{cl}"', capture_output=True, text=True, cwd=out.parent)
    if r.returncode != 0:
        pytest.fail("host build of grid_engine.h failed:\n" + r.stdout + r.stderr)
    return out


def cpp_frames(exe, name, n=FRAMES):
    out = subprocess.run([str(exe), name, str(n)], capture_output=True, text=True, check=True).stdout
    return [list(map(int, line.split())) for line in out.splitlines()]


def py_frames(name, n=FRAMES):
    eng = grid_anim.GridEngine()
    return [eng.frame(name, {}) for _ in range(n)]


@pytest.mark.parametrize("name", DETERMINISTIC)
def test_matches_python_frame_for_frame(grid_dump, name):
    cpp, py = cpp_frames(grid_dump, name), py_frames(name)
    assert len(cpp) == len(py) == FRAMES
    for i, (a, b) in enumerate(zip(cpp, py)):
        assert a == b, f"{name}: frame {i} differs (C++ {a} vs Python {b})"


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
