"""Compile a tests/host/*.cpp driver against the firmware headers with whatever C++ compiler exists."""
import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
FW_SRC = ROOT / "wfd-firmware" / "clock" / "src"


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


def build(driver: Path, out_dir: Path) -> Path:
    """Build driver -> out_dir/<name>.exe; skips the test if there's no compiler, fails on errors."""
    driver, out_dir = Path(driver).resolve(), Path(out_dir).resolve()
    out = out_dir / (driver.stem + ".exe")
    gxx = shutil.which("g++") or shutil.which("clang++")
    if gxx:
        r = subprocess.run([gxx, "-std=c++17", "-O1", "-I", str(FW_SRC), str(driver), "-o", str(out)],
                           capture_output=True, text=True)
    else:
        vcvars = _msvc_vcvars()
        if not vcvars:
            pytest.skip("no C++ compiler (MSVC, g++ or clang++)")
        cl = (f'call "{vcvars}" >nul && cl /nologo /std:c++17 /EHsc /O1 /D_USE_MATH_DEFINES '
              f'/I "{FW_SRC}" "{driver}" /Fe:"{out}"')
        # one raw string: cmd needs these quotes verbatim (a list would get them \"-escaped)
        r = subprocess.run(f'cmd /d /s /c "{cl}"', capture_output=True, text=True, cwd=out_dir)
    if r.returncode != 0:
        pytest.fail(f"host build of {driver.name} failed:\n" + r.stdout + r.stderr)
    return out


def run(exe: Path, *args) -> list[list[int]]:
    out = subprocess.run([str(exe), *map(str, args)], capture_output=True, text=True, check=True).stdout
    return [list(map(int, line.split())) for line in out.splitlines()]
