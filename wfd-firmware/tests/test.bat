@echo off
rem Run the firmware host tests (no clock or network needed). Extra args go to pytest,
rem e.g. "test.bat -m device" with WFD_CLOCK=<clock ip> set for the real-clock tests.
pushd "%~dp0"
uv run --with-requirements requirements.txt pytest %*
set rc=%errorlevel%
popd
exit /b %rc%
