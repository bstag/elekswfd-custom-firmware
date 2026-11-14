// WiFi (station + the WFD-Setup access point), NTP, mDNS and the app's UDP display stream.
#include <Arduino.h>
#include <WiFi.h>
#include <WiFiUdp.h>
#include <ESPmDNS.h>
#include <DNSServer.h>
#include <time.h>
#include "clock_state.h"
#include "clock_display.h"
#include "serial_cmds.h"
#include "weather_fetch.h"
#include "web_routes.h"
#include "wifi_link.h"

static DNSServer dns;
static bool ntpInit = false;
static uint32_t lastNtp = 0;
static WiFiUDP udp;

// The "WFD-Setup" network with the WiFi picker. At boot when no network is saved; `byButton`
// when a button was held while the saved network couldn't be joined (wrong password, new
// router). That one gives up after 10 minutes and restarts, to try the saved network again.
static uint32_t apSince = 0;
static const uint32_t AP_GIVE_UP_MS = 600000;
void startAP(bool byButton) {
  apMode = true;
  if (byButton) {
    apSince = millis() | 1;
    WiFi.setAutoReconnect(false);       // the retries would keep pulling the AP off its channel
    WiFi.disconnect();
  }
  WiFi.mode(WIFI_AP_STA);
  WiFi.scanNetworks(true);
  WiFi.softAP("WFD-Setup");
  dns.start(53, "*", WiFi.softAPIP());
  startWebRoutes();
}

// Why the last attempt to join the saved network ended (WIFI_REASON_*), set by the WiFi task.
static volatile uint8_t wifiFailReason = 0;
// The network is there but turns us away (wrong password, mostly), as opposed to not being
// found at all (router off or out of range: keep trying, it comes back by itself).
static bool wifiRejected() {
  uint8_t r = wifiFailReason;
  return r == WIFI_REASON_AUTH_EXPIRE || r == WIFI_REASON_4WAY_HANDSHAKE_TIMEOUT ||
         r == WIFI_REASON_AUTH_FAIL || r == WIFI_REASON_HANDSHAKE_TIMEOUT;
}
static const uint32_t REJECTED_MS = 180000;   // turned away for this long since boot: open the setup network

// Joining the saved network is one mechanism: auto-reconnect (wifiBegin) starts the next attempt
// from the WiFi task whenever one ends with a disconnect event. It doesn't retry some reasons (a
// wrong password) and never sees a DHCP that stalls, so when the WiFi task has been quiet for
// WIFI_STUCK_MS while we're off the network, wifiLoop() starts an attempt itself. Nothing cuts
// into an attempt that is still in flight (association, handshake or DHCP).
static volatile uint32_t wifiProgressAt = 0;   // last STA connect/disconnect event, or a pass while connected
static volatile bool wifiAssoc = false;        // associated (STA_CONNECTED), maybe still waiting for DHCP
static const uint32_t WIFI_STUCK_MS = 20000;

void wifiBegin() {
  g_tz = prefs.getString("tz", "CST6CDT,M3.2.0,M11.1.0");   // US Central w/ DST (default)
  g_lat = prefs.getString("lat", ""); g_lon = prefs.getString("lon", ""); g_wunit = prefs.getInt("wunit", 0);
  String ssid = prefs.getString("ssid", "");
  if (ssid.length()) {
    WiFi.mode(WIFI_STA);
    WiFi.setHostname("wfd-clock");
    WiFi.setSleep(false);               // keep radio responsive / stable
    WiFi.setAutoReconnect(true);
    WiFi.onEvent([](WiFiEvent_t, WiFiEventInfo_t info) {
                   wifiFailReason = info.wifi_sta_disconnected.reason;
                   wifiAssoc = false;
                   wifiProgressAt = millis();
                 }, ARDUINO_EVENT_WIFI_STA_DISCONNECTED);
    WiFi.onEvent([](WiFiEvent_t, WiFiEventInfo_t) { wifiAssoc = true; wifiProgressAt = millis(); },
                 ARDUINO_EVENT_WIFI_STA_CONNECTED);
    wifiProgressAt = millis();
    WiFi.begin(ssid.c_str(), prefs.getString("pass", "").c_str());
  } else {
    startAP();
  }
}

static void ntpToRtc() {
  struct tm t;
  if (!getLocalTime(&t, 50)) return;
  rtcSet(t.tm_year % 100, t.tm_mon + 1, t.tm_mday, t.tm_wday == 0 ? 7 : t.tm_wday,
         t.tm_hour, t.tm_min, t.tm_sec);
  ntpSynced = true;
}

void wifiLoop() {
  if (apMode) {
    dns.processNextRequest();
    web.handleClient();
    if (apSince && millis() - apSince > AP_GIVE_UP_MS) ESP.restart();
    return;
  }
  if (WiFi.status() == WL_CONNECTED) {
    wifiProgressAt = millis();          // a drop starts the WIFI_STUCK_MS wait from here
    if (!ntpInit) {
      configTzTime(g_tz.c_str(), "pool.ntp.org", "time.nist.gov", "time.google.com");
      startWebRoutes();
      ntpInit = true;
      if (MDNS.begin(HOSTNAME)) {
        MDNS.addService("http", "tcp", 80);
        MDNS.addService("wfd", "udp", UDP_PORT);
      }
      netUp = udp.begin(UDP_PORT);
    }
    web.handleClient();
    if (millis() - lastNtp > (ntpSynced ? 3600000UL : 4000UL)) {   // re-sync hourly once locked
      lastNtp = millis();
      ntpToRtc();
    }
    weatherApply();
    if (ntpSynced && (lastWxFetch == 0 || millis() - lastWxFetch > 900000UL)) {  // weather every 15 min
      if (netWeatherFetch()) lastWxFetch = millis() | 1;   // (not started: asked again next pass)
      else if (g_lat.length() == 0) lastWxFetch = millis() | 1;   // no location: nothing to fetch
    }
  } else {
    // Not connected, and the WiFi task hasn't reported anything for a while: no attempt is in
    // flight (auto-reconnect gave up, or the first join and its one retry failed), or DHCP stalled.
    if (millis() - wifiProgressAt > WIFI_STUCK_MS) {
      wifiProgressAt = millis();
      if (wifiAssoc) WiFi.reconnect();  // associated but no address: drop it and join again
      else WiFi.begin();                // idle: join with the saved network (no disconnect first)
    }
    // Never joined since power-on and the network keeps rejecting us: the saved password is
    // wrong. Open the setup network without waiting for someone to hold a button.
    static uint32_t rejectedSince = 0;
    if (ntpInit || !wifiRejected()) rejectedSince = 0;
    else if (!rejectedSince) rejectedSince = millis() | 1;
    else if (millis() - rejectedSince > REJECTED_MS) {
      Serial.println("wifi: the saved network rejects us; opening WFD-Setup");
      startAP(true);
      showInfo("JOIN WFD-SETUP WIFI", 60000);
    }
  }
}

// Live display commands from the app over WiFi. One packet per frame, e.g.
//   "WFD1 <key>\nSPEC 3 5\nICON 0\nGRID 0 0 0 8 0 0 0 1"
void netPoll() {
  static char buf[1400];
  int size;
  while ((size = udp.parsePacket()) > 0) {
    int len = udp.read(buf, sizeof buf - 1);
    if (len <= 0) continue;
    buf[len] = 0;
    char *save = nullptr, *ln = strtok_r(buf, "\n", &save);
    String head = ln ? String(ln) : String();
    head.trim();
    bool magic = head.startsWith("WFD1");
    String key = magic ? head.substring(4) : String();
    key.trim();
    if (!magic || (webPass.length() && key != webPass)) { udpRejected++; continue; }
    udpPackets++;
    while ((ln = strtok_r(nullptr, "\n", &save))) {
      String line = ln;
      line.trim();
      int sp = line.indexOf(' ');
      String cmd = sp < 0 ? line : line.substring(0, sp);
      static const char *ALLOWED[] = {"SPEC", "BARS", "MSG", "GRID", "ICON", "LOGO", "WX", "SET", "BYE"};
      for (const char *a : ALLOWED)
        if (cmd == a) { handle(line); break; }
    }
  }
}
