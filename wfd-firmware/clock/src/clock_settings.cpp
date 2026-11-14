// Settings (POST /api/set, the /control form, CFG/CFGSET over USB) and the JSON status report.
#include <Arduino.h>
#include <WiFi.h>
#include <time.h>
#include <ArduinoJson.h>
#include "clock_state.h"
#include "clock_display.h"
#include "buttons_alerts.h"
#include "standalone.h"
#include "weather_fetch.h"
#include "clock_settings.h"

// ============================= settings (JSON API + phone form) =============================
// One table of settings, shared by POST /api/set and the /control form. Names match the PC app
// where the meaning is the same; clock-only settings for "when the app is off" are standalone_*.
// applySettings() checks every key first and changes nothing unless all are valid.
const char *MSG_MODE_NAMES[] = {"weather", "text", "date", "rotate", "forecast"};   // index = SRC_* / MODE_ROTATE

static int indexOf(const char *const *names, int n, const char *s) {
  for (int i = 0; i < n; i++) if (s && strcmp(names[i], s) == 0) return i;
  return -1;
}

struct Pending {                       // multi-key settings, committed together after the loop
  int mode, rot, dwell, scroll; bool msg = false;
  int anim, fps, flag; bool grid = false;
  int gauto = -1, omauto = -1;
};

static bool intIn(JsonVariantConst v, int lo, int hi, int &out, String &err) {
  if (!v.is<int>() || v.as<int>() < lo || v.as<int>() > hi) { err = "expected an integer " + String(lo) + ".." + String(hi); return false; }
  out = v.as<int>();
  return true;
}
static bool boolOf(JsonVariantConst v, int &out, String &err) {
  if (!v.is<bool>()) { err = "expected true/false"; return false; }
  out = v.as<bool>() ? 1 : 0;
  return true;
}

// Validate one key; if `apply`, also make the change (single-key settings) or record it in `pd`.
static bool setting(const char *k, JsonVariantConst v, bool apply, Pending &pd, String &err, String &warn) {
  int i;
  if (!strcmp(k, "brightness")) {
    if (!intIn(v, 1, 16, i, err)) return false;
    if (apply && i != brightness) setBrightness(i);
  } else if (!strcmp(k, "clock_24h")) {
    if (!boolOf(v, i, err)) return false;
    if (apply && i != clock24) { clock24 = i; prefs.putInt("clk24", clock24); dirty = true; }
  } else if (!strcmp(k, "style")) {
    if (!intIn(v, 1, N_STYLES, i, err)) return false;
    if (apply) { setStyle(i); dirty = true; }
  } else if (!strcmp(k, "units")) {
    const char *u = v.as<const char *>();
    if (!v.is<const char *>() || (strcmp(u, "fahrenheit") && strcmp(u, "celsius"))) { err = "expected fahrenheit or celsius"; return false; }
    i = !strcmp(u, "celsius");
    if (apply && i != g_wunit) { g_wunit = i; prefs.putInt("wunit", g_wunit); lastWxFetch = 0; }   // refetch in new units
  } else if (!strcmp(k, "tz")) {
    String tz = v.as<const char *>();
    if (!v.is<const char *>() || tz.length() < 3 || tz.length() > 64) { err = "expected a POSIX TZ string"; return false; }
    if (apply && tz != g_tz) { g_tz = tz; prefs.putString("tz", g_tz); configTzTime(g_tz.c_str(), "pool.ntp.org", "time.nist.gov"); }
  } else if (!strcmp(k, "location")) {            // US ZIP, looked up on the clock
    String zip = v.as<const char *>(); zip.trim();
    bool ok = v.is<const char *>() && zip.length() == 5;
    for (char c : zip) ok = ok && isDigit(c);
    if (!ok) { err = "expected a 5-digit US ZIP"; return false; }
    if (apply && zip != prefs.getString("zip", "") && !setWeatherZip(zip)) warn = "ZIP lookup failed; location unchanged";
  } else if (!strcmp(k, "offline_text")) {
    if (!v.is<const char *>() || strlen(v.as<const char *>()) > MSG_TEXT_MAX) { err = "expected text up to " + String(MSG_TEXT_MAX) + " chars"; return false; }
    if (apply) setMsgText(v.as<const char *>());
  } else if (!strcmp(k, "msg_standalone")) {
    if (!boolOf(v, i, err)) return false;
    pd.omauto = i;
  } else if (!strcmp(k, "standalone_msg_mode")) {
    if ((i = indexOf(MSG_MODE_NAMES, N_SRC, v.as<const char *>())) < 0) { err = "expected weather, text, date, forecast or rotate"; return false; }
    pd.mode = i; pd.msg = true;
  } else if (!strcmp(k, "standalone_rotate")) {
    if (!v.is<JsonArrayConst>()) { err = "expected a list of weather/text/date"; return false; }
    int rot = 0;
    for (JsonVariantConst s : v.as<JsonArrayConst>()) {
      if ((i = indexOf(MSG_MODE_NAMES, N_SRC, s.as<const char *>())) < 0 || i == MODE_ROTATE) { err = "expected a list of weather/text/date/forecast"; return false; }
      rot |= 1 << i;
    }
    pd.rot = rot; pd.msg = true;
  } else if (!strcmp(k, "standalone_dwell")) {
    if (!intIn(v, 2, 600, pd.dwell, err)) return false;
    pd.msg = true;
  } else if (!strcmp(k, "standalone_scroll_ms")) {
    if (!intIn(v, 50, 5000, pd.scroll, err)) return false;
    pd.msg = true;
  } else if (!strcmp(k, "mic_standalone")) {
    if (!boolOf(v, i, err)) return false;
    if (apply && i != micAuto) { micAuto = i; prefs.putInt("micauto", micAuto); }
  } else if (!strcmp(k, "grid_standalone")) {
    if (!boolOf(v, i, err)) return false;
    pd.gauto = i;
  } else if (!strcmp(k, "standalone_anim")) {
    if ((i = grid::animFromName(v.as<const char *>() ? v.as<const char *>() : "")) < 0) { err = "unknown animation"; return false; }
    if (i == grid::CUSTOM && gaN == 0) { err = "no drawn frames stored"; return false; }
    pd.anim = i; pd.grid = true;
  } else if (!strcmp(k, "standalone_fps")) {
    if (!intIn(v, 1, 30, pd.fps, err)) return false;
    pd.grid = true;
  } else if (!strcmp(k, "standalone_frame")) {
    if (!boolOf(v, pd.flag, err)) return false;
    pd.grid = true;
  } else if (!strcmp(k, "alarms")) {           // [{"on":true,"time":"06:30","days":["mon",...]}], up to 4
    if (!v.is<JsonArrayConst>() || v.size() > extras::N_ALARMS) { err = "expected a list of up to 4 alarms"; return false; }
    extras::Alarm next[extras::N_ALARMS] = {};
    int n = 0;
    for (JsonVariantConst a : v.as<JsonArrayConst>()) {
      int hh, mm, on;
      if (!a.is<JsonObjectConst>() || !extras::parseTime(a["time"].as<const char *>(), &hh, &mm)) { err = "alarm " + String(n + 1) + ": expected \"time\": \"HH:MM\""; return false; }
      if (!boolOf(a["on"], on, err)) { err = "alarm " + String(n + 1) + ": \"on\" " + err; return false; }
      if (!a["days"].is<JsonArrayConst>()) { err = "alarm " + String(n + 1) + ": expected \"days\": [\"mon\", ...]"; return false; }
      uint8_t days = 0;
      for (JsonVariantConst d : a["days"].as<JsonArrayConst>()) {
        int di = indexOf(extras::DAY_KEYS, 7, d.as<const char *>());
        if (di < 0) { err = "alarm " + String(n + 1) + ": days are mon tue wed thu fri sat sun"; return false; }
        days |= 1 << di;
      }
      next[n++] = {(uint8_t)on, (uint8_t)hh, (uint8_t)mm, days};
    }
    if (apply) { memcpy(alarms, next, sizeof alarms); prefs.putBytes("alarms", alarms, sizeof alarms); }
  } else if (!strcmp(k, "timer")) {            // seconds to count down; 0 cancels
    if (!intIn(v, 0, 86399, i, err)) return false;
    if (apply) { cdEnd = i ? millis() + (uint32_t)i * 1000 : 0; if (!cdEnd && alertKind == ALERT_TIMER) stopAlert(); dirty = true; }
  } else if (!strcmp(k, "alert")) {            // false stops an alert; true tests one
    if (!boolOf(v, i, err)) return false;
    if (apply) { if (i) startAlert(ALERT_ALARM); else stopAlert(); }
  } else if (!strcmp(k, "weather_scene")) {
    const char *s = v.as<const char *>() ? v.as<const char *>() : "";
    i = strcmp(s, "auto") ? grid::sceneFromName(s) : -1;
    if (i < 0 && strcmp(s, "auto")) { err = "expected auto or a scene: clear, partly, cloudy, fog, rain, snow, storm, night, night_partly"; return false; }
    if (apply && i != wxScenePick) { wxScenePick = i; prefs.putInt("wxsc", wxScenePick); }
  } else {
    err = "unknown setting";
    return false;
  }
  return true;
}

// Validate everything in `in`; only if all keys are valid, apply them. Errors/warnings go to `out`.
bool applySettings(JsonObjectConst in, JsonDocument &out) {
  Pending pd;
  pd.mode = msgMode; pd.rot = msgRot; pd.dwell = msgDwell; pd.scroll = msgScroll;
  pd.anim = gaAnim; pd.fps = gaFps; pd.flag = gaFlag;
  bool ok = true;
  String err, warn;
  for (JsonPairConst kv : in)
    if (!setting(kv.key().c_str(), kv.value(), false, pd, err, warn)) { out["error"][kv.key()] = err; ok = false; }
  if (!ok) return false;
  for (JsonPairConst kv : in) {
    warn = "";
    setting(kv.key().c_str(), kv.value(), true, pd, err, warn);
    if (warn.length()) out["warning"][kv.key()] = warn;
  }
  if (pd.msg) setMsgCfg(pd.mode, pd.rot, pd.dwell, pd.scroll);
  if (pd.grid && (pd.anim != gaAnim || pd.fps != gaFps || pd.flag != gaFlag)) setGridAnim(pd.anim, pd.fps, pd.flag);
  if (pd.gauto >= 0 && pd.gauto != gaAuto) { gaAuto = pd.gauto; prefs.putInt("gauto", gaAuto); }
  if (pd.omauto >= 0 && pd.omauto != omAuto) { omAuto = pd.omauto; prefs.putInt("omauto", omAuto); }
  out["ok"] = true;
  return true;
}

// Same settings and checks as /api/set, for the PC app over USB. Replies are one line each.
void serialCfg(const String &json) {
  JsonDocument out;
  if (json.length()) {
    JsonDocument in;
    if (deserializeJson(in, json) || !in.is<JsonObject>()) {
      Serial.println("CFGERR {\"error\":\"expected a JSON object\"}");
      return;
    }
    if (!applySettings(in.as<JsonObjectConst>(), out)) {
      Serial.print("CFGERR ");
      serializeJson(out, Serial);
      Serial.println();
      return;
    }
  }
  JsonDocument cfg;
  configJson(cfg.to<JsonObject>());
  if (out["warning"].is<JsonObject>()) cfg["_warning"] = out["warning"];
  cfg["_fw"] = FW_BUILD;                         // so the app sees the version over USB too
  cfg["_protocol"] = fwProtocol;
  cfg["_ip"] = WiFi.status() == WL_CONNECTED ? WiFi.localIP().toString() : String();   // "" off WiFi
  cfg["_pw"] = webPass.length() > 0;                // a web password is set (WEBPASS)
  cfg["_ntp"] = ntpSynced;                          // has internet time: the app leaves the clock's time alone
  Serial.print(json.length() ? "CFGOK " : "CFG ");
  serializeJson(cfg, Serial);
  Serial.println();
}

// The weather grid animation's scene: the picked one, else our own latest fetch, else the 3-way
// code (also what the app sends) with day/night from the local time.
int currentScene() {
  if (wxScenePick >= 0) return wxScenePick;
  if (wxScene >= 0 && millis() - wxSceneTime < 7200000) return wxScene;
  struct tm tmv;
  bool day = !getLocalTime(&tmv, 0) || (tmv.tm_hour >= 6 && tmv.tm_hour < 20);
  return wxCode == 2 ? grid::SC_CLOUDY : wxCode == 3 ? grid::SC_RAIN : day ? grid::SC_CLEAR : grid::SC_NIGHT;
}

void configJson(JsonObject c) {
  c["brightness"] = brightness;
  c["clock_24h"] = (bool)clock24;
  c["style"] = specStyle;
  c["units"] = g_wunit ? "celsius" : "fahrenheit";
  c["tz"] = g_tz;
  c["location"] = prefs.getString("zip", "");
  c["offline_text"] = msgText;
  c["msg_standalone"] = (bool)omAuto;
  c["standalone_msg_mode"] = MSG_MODE_NAMES[msgMode];
  JsonArray rot = c["standalone_rotate"].to<JsonArray>();
  for (int i = 0; i < N_SRC; i++) if (msgRot & (1 << i)) rot.add(MSG_MODE_NAMES[i]);
  c["standalone_dwell"] = msgDwell;
  c["standalone_scroll_ms"] = msgScroll;
  c["mic_standalone"] = (bool)micAuto;
  c["grid_standalone"] = (bool)gaAuto;
  c["standalone_anim"] = grid::NAMES[gaAnim];
  c["standalone_fps"] = gaFps;
  c["standalone_frame"] = (bool)gaFlag;
  c["weather_scene"] = wxScenePick < 0 ? "auto" : grid::SCENE_NAMES[wxScenePick];
  JsonArray al = c["alarms"].to<JsonArray>();
  for (int a = 0; a < extras::N_ALARMS; a++) {
    JsonObject o = al.add<JsonObject>();
    char hm[6];
    snprintf(hm, sizeof hm, "%02d:%02d", alarms[a].hh, alarms[a].mm);
    o["on"] = (bool)alarms[a].on;
    o["time"] = hm;
    JsonArray d = o["days"].to<JsonArray>();
    for (int k = 0; k < 7; k++) if (alarms[a].days >> k & 1) d.add(extras::DAY_KEYS[k]);
  }
  c["timer"] = timerLeft();
  c["alert"] = alertKind != ALERT_NONE;
}

void stateJson(JsonObject s) {
  s["fw"] = FW_BUILD;
  s["protocol"] = fwProtocol;
  s["uptime_s"] = millis() / 1000;
  s["heap_free"] = ESP.getFreeHeap();
  uint8_t t[7];
  if (rtcRead(t)) {
    char tm[24];
    snprintf(tm, sizeof tm, "20%02d-%02d-%02d %02d:%02d:%02d", bcd2dec(t[6]), bcd2dec(t[5] & 0x1F),
             bcd2dec(t[4]), bcd2dec(t[2] & 0x3F), bcd2dec(t[1]), bcd2dec(t[0] & 0x7F));
    s["time"] = tm;
  }
  s["ntp"] = ntpSynced;
  JsonObject w = s["wifi"].to<JsonObject>();
  w["ssid"] = WiFi.SSID();
  w["ip"] = WiFi.localIP().toString();
  w["rssi"] = WiFi.RSSI();
  JsonObject wx = s["weather"].to<JsonObject>();
  wx["fresh"] = wxFresh();
  wx["temp"] = wxTemp;
  wx["unit"] = wxUnit ? "C" : "F";
  wx["cond"] = wxCode == 1 ? "Sunny" : wxCode == 2 ? "Cloudy" : wxCode == 3 ? "Rain" : "";
  wx["scene"] = grid::SCENE_NAMES[currentScene()];   // what the weather animation shows now
  if (fcFresh()) {
    JsonArray fc = wx["forecast"].to<JsonArray>();
    static const char *const DAYS3[7] = {"Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"};
    for (int d = 0; d < fcN; d++) {
      JsonObject o = fc.add<JsonObject>();
      o["day"] = DAYS3[(todayIdx() + d) % 7];
      o["hi"] = fcHi[d];
      o["lo"] = fcLo[d];
      o["scene"] = grid::SCENE_NAMES[grid::sceneFromWmo(fcCode[d], true)];
    }
  }
  s["timer"] = timerLeft();
  s["alert"] = alertKind == ALERT_TIMER ? "timer" : alertKind == ALERT_ALARM ? "alarm" : "";
  s["app_connected"] = appTime.within(APP_ON_MS);
  s["message"] = msgShown;
  s["stored_frames"] = gaN;
  if (otaLogValid) {                               // the last firmware upload (see OtaLog)
    JsonObject o = s["ota"].to<JsonObject>();
    o["result"] = otaLog.result;
    o["bytes"] = otaLog.bytes;
    o["chunks"] = otaLog.chunks;
    o["ms"] = otaLog.ms;
    o["max_gap_ms"] = otaLog.maxGapMs;
    o["gap_at"] = otaLog.gapAt;
    o["rssi_start"] = otaLog.rssiStart;
    o["rssi_end"] = otaLog.rssiEnd;
    o["heap_start"] = otaLog.heapStart;
    o["heap_end"] = otaLog.heapEnd;
  }
  JsonObject net = s["net"].to<JsonObject>();
  net["host"] = String(HOSTNAME) + ".local";
  net["udp_port"] = UDP_PORT;
  net["udp_packets"] = udpPackets;
  net["udp_rejected"] = udpRejected;
  net["key_required"] = webPass.length() > 0;
  JsonObject m = s["mic"].to<JsonObject>();
  m["active"] = millis() - micFrameAt < 500;
  m["level"] = micLevel;
  m["bass"] = micBass;
  m["treble"] = micTreble;
  JsonArray rows = s["grid_rows"].to<JsonArray>();
  const uint8_t *r = gaAnim == grid::CUSTOM ? &gaFrames[gaIdx * 7] : gEngRows;
  for (int k = 0; k < 7; k++) rows.add(r[k]);
}
