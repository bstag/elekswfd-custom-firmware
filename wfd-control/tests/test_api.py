"""The local HTTP API the dashboard and drawing tool use."""


def test_post_requires_header(app, api):
    status, _ = api("/api/set", {"logo": True}, header=False)
    assert status == 403
    assert app.cfg["logo"] is False


def test_set_normalizes_values(app, api):
    assert api("/api/set", {"brightness": "9", "logo": True, "gain_out": 2}) == (200, {"ok": True})
    assert app.cfg["brightness"] == 9
    assert app.cfg["logo"] is True
    assert app.cfg["gain_out"] == 2.0 and isinstance(app.cfg["gain_out"], float)


def test_set_rejects_bad_values_and_applies_nothing(app, api):
    status, body = api("/api/set", {"brightness": 3, "scroll_ms": "abc", "nope": 1})
    assert status == 400
    assert set(body["error"]) == {"scroll_ms", "nope"}
    assert app.cfg["brightness"] == app.DEFAULTS["brightness"]


def test_status_returns_state_and_config(app, api):
    app.set_state(message="HELLO")
    status, body = api("/api/status")
    assert status == 200
    assert body["state"]["message"] == "HELLO"
    assert body["config"]["style"] == app.DEFAULTS["style"]


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


def test_om_save_queues_cleaned_text(app, api):
    status, body = api("/api/om_save", {"text": "  HELLO {time}\n"})
    assert status == 200 and body == {"queued": 1, "text": "HELLO {time}"}
    assert app.uploads[-1] == {"type": "omtext", "text": "HELLO {time}"}
    assert api("/api/om_save", {"text": 5})[0] == 400


def test_unknown_paths_404(api):
    assert api("/nope")[0] == 404
    assert api("/api/nope", {})[0] == 404
