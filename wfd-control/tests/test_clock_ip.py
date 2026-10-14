"""The clock's WiFi address, learned over USB (CFG `_ip`) and used before any mDNS lookup."""
import pytest


def test_usb_reports_the_clock_ip(app, sim):
    sim(1).run()
    assert app.state["clock_ip"] == "10.0.0.9"
    assert "_ip" not in app.state["clock"], "the address isn't a setting"


def test_clock_off_wifi_leaves_the_ip_alone(app, sim):
    s = sim(1)
    s.version["_ip"] = ""                        # the clock isn't on WiFi
    s.run()
    assert app.state["clock_ip"] is None


def test_the_reported_address_is_used_without_a_lookup(app, monkeypatch):
    app.state["clock_ip"] = "10.0.0.9"
    monkeypatch.setattr(app.mdns, "resolve", lambda *a, **k: pytest.fail("no mDNS lookup needed"))
    monkeypatch.setattr(app.socket, "gethostbyname", lambda h: pytest.fail("no DNS lookup needed"))
    assert app.find_clock_ip() == "10.0.0.9"


def test_a_configured_ip_still_wins(app):
    app.state["clock_ip"] = "10.0.0.9"
    app.cfg["clock_host"] = "192.168.1.50"
    assert app.find_clock_ip() == "192.168.1.50"


def test_a_stale_address_is_forgotten(app, monkeypatch):
    app.state["clock_ip"] = "10.0.0.9"

    def gone(self):
        raise app.LinkLost("timed out")
    monkeypatch.setattr(app.WifiLink, "get_config", gone)
    assert app.open_wifi() is None
    assert app.state["clock_ip"] is None, "next time: look it up by name"
