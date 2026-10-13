"""ClockLink (the serial worker) on simulated time against a fake serial port."""
import difflib
import os
from pathlib import Path

import pytest

GOLDEN = Path(__file__).parent / "golden" / "clocklink.log"


def busy_scenario(app, sim):
    """45 s of everything at once: rotate mode, icons, grid, uploads, a preview, a sync, an unplug."""
    app.cfg.update(message_mode="rotate", rotate_weather=True, rotate_window=True, rotate_text=True,
                   rotate_date=True, rotate_dwell=6, custom_text="HI {time} {day}", icon_mode="system",
                   grid_enable=True, grid_anim="spin", grid_frame=True, grid_fps=12, logo=True)
    app.uploads.append({"type": "omtext", "text": "WFD {time}"})

    def preview():
        app.grid_preview.update(rows=[1, 2, 4, 8, 16, 32, 64], frame=1, at=app.time.time())

    def grid_upload():
        app.uploads.append({"type": "grid", "frames": [[1] * 7, [2] * 7, [4] * 7], "fps": 9, "frame": True})

    return sim(45, unplug_after=400,
               events={20: preview, 30: grid_upload, 35: app.sync_now.set})


def test_matches_golden_log(app, sim):
    """Every line sent in the busy scenario matches the recorded log.

    If you changed what the app sends on purpose, regenerate it and review the diff:
        set WFD_UPDATE_GOLDEN=1 && uv run ... pytest tests/test_clocklink.py
    """
    lines = busy_scenario(app, sim).run()
    if os.environ.get("WFD_UPDATE_GOLDEN"):
        GOLDEN.parent.mkdir(exist_ok=True)
        GOLDEN.write_text("\n".join(lines) + "\n", encoding="utf-8")
        pytest.skip(f"golden log rewritten ({len(lines)} lines)")
    expected = GOLDEN.read_text(encoding="utf-8").splitlines()
    if lines != expected:
        diff = list(difflib.unified_diff(expected, lines, "golden", "now", lineterm="", n=2))
        pytest.fail("serial output changed:\n" + "\n".join(diff[:60]))


def test_reconnect_resends_every_setting(app, sim):
    s = sim(10, unplug_after=200)
    s.run()
    assert s.opens == 2
    for port in ("COM1", "COM2"):
        sent = s.commands(port)
        for cmd in ("SET", "STYLE", "BRIGHT", "CLK24", "GAUTO", "OMAUTO", "GEO"):
            assert cmd in sent, f"{cmd} not sent on {port}"


def test_settings_only_sent_when_changed(app, sim):
    def brighter():
        app.cfg["brightness"] = 11

    s = sim(4, events={2: brighter})
    s.run()
    bright = [line.split(maxsplit=2)[2] for line in s.lines if " BRIGHT " in line]
    assert bright == ["BRIGHT 6", "BRIGHT 11"]


def test_uploads_send_clear_add_commit_then_reassert_auto(app, sim):
    app.cfg["grid_standalone"] = False
    app.uploads.append({"type": "grid", "frames": [[1] * 7, [2] * 7], "fps": 12, "frame": True})
    s = sim(1)
    s.run()
    sent = [line.split(maxsplit=2)[2] for line in s.lines]
    i = sent.index("GCLR")
    assert sent[i:i + 4] == ["GCLR", "GADD 1 1 1 1 1 1 1", "GADD 2 2 2 2 2 2 2", "GCOMMIT 12 1"]
    assert "GAUTO 0" in sent[i + 4:], "GCOMMIT turns auto on; the user's 'off' must be re-sent"
    assert app.state["saved"] == "grid: 2 frames"


def test_offline_text_is_sent_as_text(app, sim):
    app.uploads.append({"type": "omtext", "text": "HELLO {day}"})
    s = sim(1)
    s.run()
    assert [line.split(maxsplit=2)[2] for line in s.lines if " OMTEXT " in line] == ["OMTEXT HELLO {day}"]
    assert app.state["saved"] == "message: 'HELLO {day}'"


def test_temp_placeholder(app, sim):
    app.cfg.update(message_mode="text", custom_text="{temp}")
    s = sim(1)
    s.run()
    assert app.state["message"] == "92F"


def test_sync_now_pushes_time(app, sim):
    s = sim(5, events={3: app.sync_now.set})
    s.run()
    sets = [line for line in s.lines if " SET " in line]
    assert len(sets) == 2 and sets[1].startswith("   3.")
    assert not app.sync_now.is_set()


def test_spectrum_off_sends_zero_levels(app, sim):
    app.cfg["spectrum"] = False
    s = sim(2)
    s.run()
    assert {line.split(maxsplit=2)[2] for line in s.lines if " SPEC " in line} == {"SPEC 0 0"}
    assert app.state["levels"]["out"] > 0, "levels are still computed for the icon row"


def test_weather_mode_leaves_message_line_to_firmware(app, sim):
    app.cfg["message_mode"] = "weather"
    s = sim(3)
    s.run()
    assert "MSG" not in s.commands()
    assert "WX" in s.commands()
