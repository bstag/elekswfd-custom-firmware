"""Settings validation, message rendering and icon logic."""
import pytest


@pytest.mark.parametrize("key, value, expected", [
    ("brightness", "6", 6),
    ("brightness", 99, 16),            # clamped to the TM1680's range
    ("brightness", 0, 1),
    ("style", 7.9, 5),
    ("scroll_ms", 10, 50),
    ("gain_mic", "1.5", 1.5),
    ("message_mode", "date", "date"),
    ("grid_custom", [[1, 2], [300] * 9], [[1, 2, 0, 0, 0, 0, 0], [300 & 0x7F] * 7]),
])
def test_coerce_accepts_and_normalizes(app, key, value, expected):
    assert app.coerce(key, value) == expected


@pytest.mark.parametrize("key, value", [
    ("brightness", "abc"),
    ("brightness", True),              # bools are not numbers here
    ("brightness", None),
    ("logo", "yes"),
    ("message_mode", "bogus"),
    ("units", "kelvin"),
    ("custom_text", 5),
    ("grid_custom", [1, 2]),
])
def test_coerce_rejects(app, key, value):
    with pytest.raises(ValueError):
        app.coerce(key, value)


def test_every_default_passes_its_own_validation(app):
    for key, value in app.DEFAULTS.items():
        assert app.coerce(key, value) == value, key


@pytest.mark.parametrize("text", ["HI", "SIXCHR", "HELLO WORLD", "x" * 40])
def test_offline_frames_match_live_scroll(app, text):
    frames = app.render_om_frames(text)
    assert len(frames) == app.scroll_len(text)
    assert frames == [app.scroll_frame(text, i) for i in range(len(frames))]
    assert all(len(f) == 12 for f in frames)


def test_scroll_wraps_with_gap(app):
    text = "HELLO WORLD"
    assert app.scroll_frame(text, 0) == app.scroll_frame(text, len(text) + 3)


def test_short_text_does_not_scroll(app):
    assert app.render_om_frames("") == [app.font14.render6(" " * 6, 0)]
    assert len(app.render_om_frames("ABCDEF")) == 1


def test_icons_all_and_off(app):
    levels, metrics = {"out": 12, "mic": 12}, {"cpu": 99, "gpu": 99, "ram": 99}
    assert app.compute_icons("all", levels, metrics, True, 5) == 0x1FFF
    assert app.compute_icons("off", levels, metrics, True, 5) == 0


def test_icons_audio_meter(app):
    bit = lambda *names: sum(1 << app.ICON[n] for n in names)
    m = {"cpu": 0, "gpu": 0, "ram": 0}
    assert app.compute_icons("audio", {"out": 0, "mic": 0}, m, False, None) == 0
    assert app.compute_icons("audio", {"out": 5, "mic": 3}, m, False, None) == bit("speaker", "headphones", "mic")


def test_repeatable_animations_exist(app):
    """GridEngine silently falls back to 'spin' for unknown names, so check the names are real."""
    for name in app.REPEATABLE:
        assert hasattr(app.grid_anim.GridEngine, "a_" + name), name
