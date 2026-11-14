"""Keep the build stamp current: FW_BUILD in src/main.cpp is "wfd-clock " __DATE__ " " __TIME__
of main.cpp's own compile, and the PC app and make_release.py go by it. So main.cpp is rebuilt
whenever any other source file changes (as when it was the only one).

PlatformIO pre-build script (extra_scripts in platformio.ini).
"""
from pathlib import Path

Import("env")  # noqa: F821

src = Path(env.subst("$PROJECT_SRC_DIR"))  # noqa: F821
others = [str(p) for p in sorted(src.iterdir()) if p.suffix in (".cpp", ".h") and p.name != "main.cpp"]
env.Depends("$BUILD_DIR/src/main.cpp.o", others)  # noqa: F821
