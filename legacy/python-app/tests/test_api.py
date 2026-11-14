"""The local HTTP API the dashboard and drawing tool use."""
import pytest


def test_post_requires_header(app, api):
    status, _ = api("/api/set", {"logo": True}, header=False)
    assert status == 403
    assert app.cfg["logo"] is False


def test_set_normalizes_values(app, api):
    assert api("/api/set", {"grid_fps": "9", "logo": True, "gain_out": 2}) == (200, {"ok": True})
    assert app.cfg["grid_fps"] == 9
    assert app.cfg["logo"] is True
    assert app.cfg["gain_out"] == 2.0 and isinstance(app.cfg["gain_out"], float)


def test_set_rejects_bad_values_and_applies_nothing(app, api):
    status, body = api("/api/set", {"grid_fps": 3, "scroll_ms": "abc", "nope": 1})
    assert status == 400
    assert set(body["error"]) == {"scroll_ms", "nope"}
    assert app.cfg["grid_fps"] == app.DEFAULTS["grid_fps"]


def test_status_returns_state_and_config(app, api):
    app.set_state(message="HELLO", clock={"brightness": 5, "style": 2})
    status, body = api("/api/status")
    assert status == 200
    assert body["state"]["message"] == "HELLO"
    assert body["config"]["grid_fps"] == app.DEFAULTS["grid_fps"]
    assert (body["config"]["brightness"], body["config"]["style"]) == (5, 2), "the clock's own settings"


@pytest.fixture
def clock_calls(app, monkeypatch):
    """Replace the trip to the clock: record patches, answer with clock_calls.reply."""
    class Calls(list):
        reply = (True, None)
    calls = Calls()

    def fake(patch, timeout=3.0):
        calls.append(patch)
        return calls.reply
    monkeypatch.setattr(app, "set_clock", fake)
    return calls


def test_clock_settings_go_to_the_clock(app, api, clock_calls):
    assert api("/api/set", {"brightness": 5, "standalone_anim": "rain", "logo": True}) == (200, {"ok": True})
    assert clock_calls == [{"brightness": 5, "standalone_anim": "rain"}]
    assert app.cfg["logo"] is True
    assert "brightness" not in app.cfg, "the app no longer keeps its own copy"


def test_clock_rejection_applies_nothing(app, api, clock_calls):
    clock_calls.reply = (False, {"brightness": "expected an integer 1..16"})
    status, body = api("/api/set", {"brightness": 99, "logo": True})
    assert status == 400 and body["error"] == {"brightness": "expected an integer 1..16"}
    assert app.cfg["logo"] is False


def test_app_errors_stop_before_the_clock(app, api, clock_calls):
    status, _ = api("/api/set", {"brightness": 5, "grid_fps": "x"})
    assert status == 400 and clock_calls == []


def test_clock_warnings_are_passed_on(app, api, clock_calls):
    clock_calls.reply = (True, {"location": "ZIP lookup failed; location unchanged"})
    assert api("/api/set", {"location": "00000"}) == (200, {"ok": True, "warning": {"location": "ZIP lookup failed; location unchanged"}})


def test_clock_settings_need_the_clock(app, api):
    app.set_state(connected=False)
    status, body = api("/api/set", {"brightness": 5})
    assert status == 400 and body["error"] == {"brightness": "clock not connected"}


def test_sync_sets_event(app, api):
    assert api("/api/sync", {})[0] == 200
    assert app.sync_now.is_set()


def test_grid_preview_masks_rows(app, api):
    api("/api/grid_preview", {"rows": [127, 300], "frame": 1})
    assert app.grid_preview["rows"] == [127, 300 & 0x7F, 0, 0, 0, 0, 0]
    assert app.grid_preview["frame"] == 1


def test_grid_save_queues_validated_frames(app, api):
    assert api("/api/grid_save", {"frames": [[1, 2, 3]], "fps": 10}) == (200, {"queued": 1})
    assert app.uploads[-1]["frames"] == [[1, 2, 3, 0, 0, 0, 0]]
    assert api("/api/grid_save", {"frames": "junk"})[0] == 400


def test_grid_save_builtin_sends_name_not_frames(app, api):
    app.cfg.update(grid_anim="rain", grid_fps=14, grid_frame=True)
    assert api("/api/grid_save", {}) == (200, {"queued": 1})
    assert app.uploads[-1] == {"type": "ganim", "name": "rain", "fps": 14, "frame": True}


def test_grid_save_custom_needs_frames(app, api):
    app.cfg.update(grid_anim="custom", grid_custom=[])
    assert api("/api/grid_save", {})[0] == 400
    app.cfg["grid_custom"] = [[1] * 7, [2] * 7]
    assert api("/api/grid_save", {}) == (200, {"queued": 2})
    assert app.uploads[-1]["type"] == "grid"


def test_grid_save_refuses_live_only_animation(app, api):
    status, body = api("/api/grid_save", {"anim": "cpu"})
    assert status == 400 and "live data" in body["error"]


def test_om_save_sets_the_clocks_text(app, api, clock_calls):
    status, body = api("/api/om_save", {"text": "  HELLO {time}\n"})
    assert status == 200 and body == {"queued": 1, "text": "HELLO {time}"}
    assert clock_calls == [{"offline_text": "HELLO {time}"}]
    assert api("/api/om_save", {"text": 5})[0] == 400


def test_unknown_paths_404(api):
    assert api("/nope")[0] == 404
    assert api("/api/nope", {})[0] == 404


def test_old_config_files_dont_bring_back_clock_settings(app, tmp_path):
    app.CONFIG = tmp_path / "old.json"
    app.CONFIG.write_text('{"brightness": 3, "style": 1, "logo": true, "_comment": "x"}')
    loaded = app.load_config()
    assert loaded["logo"] is True
    assert not {"brightness", "style", "_comment"} & set(loaded)
