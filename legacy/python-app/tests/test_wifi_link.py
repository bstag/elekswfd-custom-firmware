"""The app driving the clock over WiFi (WifiLink): UDP for the live display, HTTP for the rest."""
import re
import threading
from pathlib import Path

MAIN_CPP = Path(__file__).resolve().parents[2] / "wfd-firmware" / "clock" / "src" / "main.cpp"


def sent_cmds(s):
    return [line.split(maxsplit=2)[2].split()[0] for line in s.lines]


def test_one_packet_per_frame_with_the_header(app, sim):
    s = sim(3, wifi=True)
    s.run()
    assert app.state["port"] == "WiFi 10.0.0.9"
    assert 80 <= len(s.packets) <= 95, "about 30 packets a second"
    assert all(head == "WFD1" for head, _ in s.packets)
    first = [ln.split()[0] for ln in s.packets[0][1]]
    assert {"SPEC", "ICON", "LOGO"} <= set(first), "a frame's commands travel together"
    assert all(len("\n".join([h] + ls)) < 1400 for h, ls in s.packets), "fits one datagram"


def test_password_goes_in_the_header(app, sim):
    s = sim(1, wifi=True, password="hunter2")
    s.run()
    assert {head for head, _ in s.packets} == {"WFD1 hunter2"}


def test_only_display_commands_go_over_udp(app, sim):
    app.state["geo"] = {"lat": 1.0, "lon": 2.0, "unit": 0}
    s = sim(12, wifi=True)
    s.clock_cfg["location"] = ""                     # would get GEO over USB
    s.run()
    allowed = set(re.findall(r'"(\w+)"', re.search(r"ALLOWED\[\] = \{([^}]*)\}", MAIN_CPP.read_text(encoding="utf-8")).group(1)))
    assert {"SPEC", "GRID", "BYE"} <= allowed                # read from the firmware's netPoll()
    assert set(sent_cmds(s)) <= allowed
    assert "SET" not in sent_cmds(s), "a WiFi clock with NTP keeps its own time"


def test_time_is_sent_if_the_clock_has_no_internet_time(app, sim):
    s = sim(2, wifi=True)
    s.clock_ntp = False
    s.run()
    assert "SET" in sent_cmds(s)


def test_settings_and_health_use_http(app, sim):
    ok = [{"brightness": 9}, threading.Event(), None]
    bad = [{"brightness": 99}, threading.Event(), None]
    s = sim(12, wifi=True, events={2: lambda: app.clock_requests.extend([ok, bad])})
    s.run()
    assert ok[2] == (True, None) and app.state["clock"]["brightness"] == 9
    assert bad[2] == (False, {"brightness": "expected an integer 1..16"})
    gets = [c for c in s.http_calls if c[1:3] == ("GET", "/api/status")]
    assert len(gets) >= 3, "settings re-read on connect, every 5 s, after a change"


def test_uploads_use_http(app, sim):
    app.uploads.extend([{"type": "ganim", "name": "rain", "fps": 9, "frame": True},
                        {"type": "grid", "frames": [[1] * 7, [2] * 7], "fps": 5, "frame": False}])
    s = sim(1, wifi=True)
    s.run()
    posts = [(c[2], c[3]) for c in s.http_calls if c[1] == "POST"]
    assert ("/api/set", {"standalone_anim": "rain", "standalone_fps": 9, "standalone_frame": True,
                         "grid_standalone": True}) in posts
    assert ("/api/frames", {"frames": [[1] * 7, [2] * 7], "fps": 5, "frame": False}) in posts
    assert "GCLR" not in sent_cmds(s)


def test_reconnects_after_the_clock_drops_off(app, sim):
    s = sim(40, wifi=True)

    def down():
        s.wifi_down = True

    def up():
        s.wifi_down = False

    s.events = [(5, down), (25, up)]
    s.run()
    assert s.wifi_opens == 2
    assert app.state["connected"] and app.state["link"] == "wifi"
    late = [line for line in s.lines if 26 < float(line.split()[0]) < 40]
    assert late, "streaming again after the clock came back"


def test_auto_switches_to_usb_when_plugged_in(app, sim):
    s = sim(20, wifi=True)

    def plug():
        s.usb_present = True

    s.events = [(3, plug)]
    s.run()
    assert app.state["link"] == "usb"
    usb_after = [line for line in s.lines if " COM" in line]
    assert usb_after and float(usb_after[0].split()[0]) < 3 + 11, "switches within ~10 s"


def test_wifi_mode_ignores_usb(app, sim):
    app.cfg["link"] = "wifi"
    s = sim(15, wifi=True)
    s.usb_present = True
    s.run()
    assert app.state["link"] == "wifi" and s.opens == 0


def test_bye_when_the_app_stops(app, sim):
    s = sim(1, wifi=True)
    s.run()
    assert s.packets[-1][1] == ["BYE"]


def test_switching_to_wifi_takes_effect_at_once(app, sim):
    s = sim(4, wifi=True)
    s.usb_present = True                        # starts on USB

    def to_wifi():
        app.cfg["link"] = "wifi"

    s.events = [(2, to_wifi)]
    s.run()
    assert app.state["link"] == "wifi"
    first_wifi = next(float(line.split()[0]) for line in s.lines if " WIFI " in line)
    assert 2 <= first_wifi < 2.5
