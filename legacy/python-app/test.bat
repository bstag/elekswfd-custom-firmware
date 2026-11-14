@echo off
rem Run the retired Python app's test suite (no clock or network needed). Extra args go to pytest.
pushd "%~dp0"
uv run --with-requirements requirements.txt --with pytest pytest %*
set rc=%errorlevel%
popd
exit /b %rc%
