"""ClockLink (the serial worker) on simulated time against a fake serial port."""
import difflib
import os
import threading
from pathlib import Path

import pytest

GOLDEN = Path(__file__).parent / "golden" / "clocklink.log"


def busy_scenario(app, sim):
    """45 s of everything at once: rotate mode, icons, grid, uploads, a preview, a sync, an unplug."""
    app.cfg.update(message_mode="rotate", rotate_weather=True, rotate_window=True, rotate_text=True,
                   rotate_date=True, rotate_dwell=6, custom_text="HI {time} {day}", icon_mode="system",
                   grid_enable=True, grid_anim="spin", grid_frame=True, grid_fps=12, logo=True)

    def preview():
        app.grid_preview.update(rows=[1, 2, 4, 8, 16, 32, 64], frame=1, at=app.time.time())

    def grid_upload():
        app.uploads.append({"type": "grid", "frames": [[1] * 7, [2] * 7, [4] * 7], "fps": 9, "frame": True})

    def clock_change():                    # e.g. brightness changed in the app's UI
        app.clock_requests.append([{"brightness": 9}, threading.Event(), None])

    return sim(45, unplug_after=400,
               events={20: preview, 25: clock_change, 30: grid_upload, 35: app.sync_now.set})


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


def test_reconnect_reads_the_clock_and_overwrites_nothing(app, sim):
    """The clock owns its settings: on (re)connect the app asks for them and pushes none."""
    s = sim(10, unplug_after=200)
    s.clock_cfg["brightness"] = 3              # changed on the clock's dashboard
    s.run()
    assert s.opens == 2
    for port in ("COM1", "COM2"):
        sent = s.commands(port)
        assert "SET" in sent and "CFG" in sent, port
        for cmd in ("STYLE", "BRIGHT", "CLK24", "GAUTO", "OMAUTO", "GEO", "CFGSET"):
            assert cmd not in sent, f"{cmd} sent on {port}"
    assert app.state["clock"]["brightness"] == 3


def test_changes_made_on_the_clock_show_up(app, sim):
    s = sim(12)

    def dashboard_change():
        s.clock_cfg["standalone_anim"] = "rain"

    s.events = [(3, dashboard_change)]
    s.run()
    assert app.state["clock"]["standalone_anim"] == "rain", "re-read every 5 s"


def test_location_goes_to_the_clock_only_if_it_has_none(app, sim):
    app.state["geo"] = {"lat": 31.5, "lon": -97.1, "unit": 0}
    s = sim(2)
    s.run()
    assert "GEO" not in s.commands()
    app.stop.clear()
    s = sim(2)
    s.clock_cfg["location"] = ""
    s.run()
    assert "GEO" in s.commands()


def test_clock_settings_round_trip(app, sim):
    ok = [{"brightness": 9, "standalone_anim": "heart"}, threading.Event(), None]
    bad = [{"brightness": 99}, threading.Event(), None]
    app.clock_requests.extend([ok, bad])
    s = sim(1)
    s.run()
    sent = [line.split(maxsplit=2)[2] for line in s.lines]
    assert 'CFGSET {"brightness":9,"standalone_anim":"heart"}' in sent
    assert ok[1].is_set() and ok[2] == (True, None)
    assert bad[1].is_set() and bad[2] == (False, {"brightness": "expected an integer 1..16"})
    assert app.state["clock"]["brightness"] == 9 and app.state["clock"]["standalone_anim"] == "heart"


def test_uploads_send_clear_add_commit_then_reread_settings(app, sim):
    app.uploads.append({"type": "grid", "frames": [[1] * 7, [2] * 7], "fps": 12, "frame": True})
    s = sim(1)
    s.run()
    sent = [line.split(maxsplit=2)[2] for line in s.lines]
    i = sent.index("GCLR")
    assert sent[i:i + 4] == ["GCLR", "GADD 1 1 1 1 1 1 1", "GADD 2 2 2 2 2 2 2", "GCOMMIT 12 1"]
    assert "CFG" in sent[i + 4:], "GCOMMIT changes the clock's settings: read them again"
    assert "GAUTO" not in s.commands()
    assert app.state["saved"] == "grid: 2 frames"


def test_builtin_grid_animation_is_sent_by_name(app, sim):
    app.uploads.append({"type": "ganim", "name": "life", "fps": 6, "frame": False})
    s = sim(1)
    s.run()
    sent = [line.split(maxsplit=2)[2] for line in s.lines]
    assert "GANIM life 6 0" in sent
    assert "CFG" in sent[sent.index("GANIM life 6 0"):], "GANIM changes the clock's settings: read them again"
    assert "GCLR" not in sent


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
