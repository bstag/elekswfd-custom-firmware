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


@pytest.mark.parametrize("raw, clean", [
    ("  HELLO  ", "HELLO"),
    ("tab\there\nnewline", "tab here newline"),  # one line only: the serial protocol is line-based
    ("café ☃", "caf"),                   # non-ASCII has no glyph -> space, then trimmed
    ("x" * 200, "x" * 96),
])
def test_clean_omtext(app, raw, clean):
    assert app.clean_omtext(raw) == clean


def test_scroll_wraps_with_gap(app):
    text = "HELLO WORLD"
    assert app.scroll_frame(text, 0) == app.scroll_frame(text, len(text) + 3)


def test_short_text_does_not_scroll(app):
    assert app.scroll_frame("ABCDEF", 0) == app.scroll_frame("ABCDEF", 5)
    assert all(len(app.scroll_frame("HELLO WORLD", i)) == 12 for i in range(20))


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
