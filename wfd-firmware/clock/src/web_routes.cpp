// The web server: the device pages (setup, control, status, update) and the JSON API.
#include <Arduino.h>
#include <WiFi.h>
#include <Update.h>
#include <ArduinoJson.h>
#include "clock_state.h"
#include "clock_display.h"
#include "clock_settings.h"
#include "nvs_store.h"
#include "ota_update.h"
#include "standalone.h"
#include "weather_fetch.h"
#include "web_pages.h"
#include "web_routes.h"

WebServer web(80);

static String esc(const String &s) {   // HTML-escape text going into a page or attribute
  String o;
  o.reserve(s.length() + 8);
  for (char c : s) {
    switch (c) {
      case '&': o += "&amp;"; break;
      case '<': o += "&lt;"; break;
      case '>': o += "&gt;"; break;
      case '"': o += "&quot;"; break;
      case '\'': o += "&#39;"; break;
      default: o += c;
    }
  }
  return o;
}

// Optional web password (serial: WEBPASS <pass>, empty = open). Guards the pages that change
// settings or flash firmware once the clock is on your LAN; the first-run setup AP stays open.
static bool webAuthed() {
  if (net.apMode) return true;
  if (net.webPass.length() == 0 || web.authenticate("admin", net.webPass.c_str())) return true;
  web.requestAuthentication();
  return false;
}

// Shared shell for every device page: charset (the pages use non-ASCII), viewport, one stylesheet.
static const char PAGE_CSS[] =
    "body{font-family:sans-serif;background:#0e1013;color:#e6e9ee;padding:18px;max-width:480px;margin:auto}"
    "a{color:#72d3ff}b{color:#72d3ff}.dim{color:#8b95a3}.warn{color:#ff5d5d}"
    "label{display:block;color:#8b95a3;margin-top:14px;font-size:13px}"
    "input,select,button{width:100%;font-size:16px;padding:9px;margin-top:4px;box-sizing:border-box;"
    "border-radius:6px;border:1px solid #2a3038;background:#1d2228;color:#eee}"
    "button{margin-top:18px;padding:12px;border:0;background:#72d3ff;color:#06212c;font-weight:600}"
    ".row{display:flex;gap:10px;align-items:center;margin-top:14px}.row input{width:auto}.row label{margin:0}"
    ".net{display:flex;align-items:center;gap:10px;padding:11px 12px;margin-top:6px;border:1px solid #2a3038;"
    "border-radius:6px;background:#1d2228;color:#e6e9ee;font-size:16px;cursor:pointer}"
    ".net input{width:auto;margin:0}.net span{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}"
    ".net:has(input:checked){border-color:#72d3ff}.sig{color:#72d3ff;letter-spacing:-2px}";

// An HTML reply sent in pieces (chunked) instead of built up as one big String, which would
// fragment the heap over a long uptime. Collects ~1 KB per piece. Starts with the shared shell
// (charset, viewport, title, stylesheet); the reply is finished by end() or when it goes out of
// scope, so call end() before anything that doesn't return (a restart).
class HtmlOut {
 public:
  HtmlOut(int code, const char *title, const char *head = "") {
    web.setContentLength(CONTENT_LENGTH_UNKNOWN);
    web.send(code, "text/html", "");
    *this << "<!doctype html><meta charset=utf-8>"
             "<meta name=viewport content='width=device-width,initial-scale=1'>"
          << head << "<title>" << title << "</title><style>" << PAGE_CSS << "</style>";
  }
  ~HtmlOut() { end(); }
  HtmlOut &operator<<(const char *s) { add(s, strlen(s)); return *this; }
  HtmlOut &operator<<(const String &s) { add(s.c_str(), s.length()); return *this; }
  HtmlOut &operator<<(int v) { char b[12]; snprintf(b, sizeof b, "%d", v); return *this << b; }
  void end() {
    if (done_) return;
    done_ = true;
    flush();
    web.sendContent("");                // the last (empty) chunk
  }

 private:
  static char buf_[1024];               // one reply at a time: the web server runs in loop()
  size_t n_ = 0;
  bool done_ = false;
  void flush() { if (n_) web.sendContent(buf_, n_); n_ = 0; }
  void add(const char *s, size_t len) {
    while (len) {
      size_t k = min(len, sizeof buf_ - n_);
      memcpy(buf_ + n_, s, k);
      n_ += k; s += k; len -= k;
      if (n_ == sizeof buf_) flush();
    }
  }
};
char HtmlOut::buf_[1024];

// A whole page from fixed text.
static void sendPage(int code, const char *title, const char *body, const char *head = "") {
  HtmlOut o(code, title, head);
  o << body;
}

static const char *signalBars(int rssi) {
  return rssi > -55 ? "▂▄▆█" : rssi > -67 ? "▂▄▆" : rssi > -78 ? "▂▄" : "▂";
}

// WiFi picker: tap a scanned network (strongest first) or type one under "Other".
// Works without JS; the inline script only focuses the right field and hides the
// password box for open networks.
static void sendSetupPage() {
  int n = WiFi.scanComplete();
  if (n == WIFI_SCAN_FAILED) { WiFi.scanNetworks(true); n = WIFI_SCAN_RUNNING; }
  if (n == WIFI_SCAN_RUNNING)
    return sendPage(200, "WFD WiFi Setup", "<h2>WFD Clock WiFi</h2><p>Scanning for networks…</p>",
                    "<meta http-equiv=refresh content=2>");

  // dedupe by SSID (keep the strongest), drop hidden ones, sort by signal
  const int MAXN = 20;
  int idx[MAXN], m = 0;
  for (int i = 0; i < n; i++) {
    if (WiFi.SSID(i).length() == 0) continue;
    int dup = -1;
    for (int k = 0; k < m; k++) if (WiFi.SSID(idx[k]) == WiFi.SSID(i)) { dup = k; break; }
    if (dup >= 0) { if (WiFi.RSSI(i) > WiFi.RSSI(idx[dup])) idx[dup] = i; continue; }
    if (m < MAXN) idx[m++] = i;
  }
  for (int a = 1; a < m; a++)                     // insertion sort, strongest first
    for (int b = a; b > 0 && WiFi.RSSI(idx[b]) > WiFi.RSSI(idx[b - 1]); b--) {
      int tmp = idx[b]; idx[b] = idx[b - 1]; idx[b - 1] = tmp;
    }

  String current = prefString(P_SSID);
  HtmlOut o(200, "WFD WiFi Setup");
  o << "<h2>WFD Clock WiFi</h2><form method=POST action=/save>"
       "<label>Choose a network <a href=/rescan style=float:right>↻ Scan again</a></label>";
  for (int k = 0; k < m; k++) {
    int i = idx[k];
    bool open = WiFi.encryptionType(i) == WIFI_AUTH_OPEN;
    String s = esc(WiFi.SSID(i));
    o << "<label class=net><input type=radio name=ssid value='" << s << "' data-open=" << (open ? "1" : "0")
      << (WiFi.SSID(i) == current ? " checked" : "") << "><span>" << s << "</span>"
      << (open ? "" : "🔒") << "<b class=sig>" << signalBars(WiFi.RSSI(i)) << "</b></label>";
  }
  if (m == 0) o << "<p class=dim>No networks found.</p>";
  o << "<label class=net><input type=radio name=ssid value='' id=oth data-open=0><span>Other network…</span></label>"
       "<input name=other id=othname placeholder='Network name (hidden networks)' autocomplete=off>";

  o << "<div id=pw><label>Password</label><input name=pass id=pass type=password placeholder='WiFi password'></div>"
       "<button type=submit>Save &amp; Connect</button></form>"
       "<p class=dim>The clock restarts and joins the network. 2.4 GHz only.</p>"
       "<script>"
       "const f=document.forms[0],pw=document.getElementById('pw'),on=document.getElementById('othname');"
       "f.onchange=e=>{const r=f.querySelector('input[name=ssid]:checked');if(!r)return;"
       "pw.style.display=r.dataset.open=='1'?'none':'';"
       "if(e.target==r)(r.id=='oth'?on:document.getElementById('pass')).focus()};"
       "on.onfocus=()=>{document.getElementById('oth').checked=true;f.onchange({})};"
       "</script>";
}

static void handleSave() {
  if (!webAuthed()) return;
  String ssid = web.arg("ssid");
  if (ssid.length() == 0) { ssid = web.arg("other"); ssid.trim(); }   // typed under "Other network…"
  if (ssid.length() == 0) { web.sendHeader("Location", "/wifi"); web.send(303); return; }
  prefPutString(P_SSID, ssid);
  prefPutString(P_PASS, web.arg("pass"));
  sendPage(200, "WFD WiFi Setup",
           "<h2>Saved — connecting…</h2><p>Rejoin your normal WiFi. The clock is restarting.</p>"
           "<p>Then open <b>http://wfd-clock.local</b>. If that doesn't load, hold any button on the clock "
           "for 2 seconds: it scrolls its address (or opens this setup again if it couldn't join).</p>");
  delay(900);
  ESP.restart();
}

static void sendStatusPage() {
  uint8_t t[7];
  char tm[32] = "—";
  if (rtcRead(t))
    snprintf(tm, sizeof tm, "20%02d-%02d-%02d %02d:%02d:%02d",
             bcd2dec(t[6]), bcd2dec(t[5] & 0x1F), bcd2dec(t[4]),
             bcd2dec(t[2] & 0x3F), bcd2dec(t[1]), bcd2dec(t[0] & 0x7F));
  HtmlOut o(200, "WFD Clock");
  o << "<h2>WFD Clock</h2><p class=dim>" << esc(FW_BUILD) << "</p>";
  o << "<p>Time: <b>" << tm << "</b></p>";
  o << "<p>NTP: <b>" << (net.ntpSynced ? "synced" : "waiting…") << "</b> &nbsp; WiFi: <b>" << esc(WiFi.SSID())
    << "</b> (" << WiFi.localIP().toString() << ", " << (int)WiFi.RSSI() << " dBm)</p>";
  const char *cond = wx.code == 1 ? "Sunny" : wx.code == 2 ? "Cloudy" : wx.code == 3 ? "Rain" : "—";
  o << "<p>Weather: <b>";
  if (wxFresh()) o << wx.temp << (wx.unit ? "C " : "F ") << cond;
  else o << "—";
  o << "</b> &nbsp; loc ";
  if (wx.lat.length()) o << wx.lat << "," << wx.lon;
  else o << "unset";
  o << "</p>";
  o << "<p><a href=/ style=font-weight:600>⚙ Dashboard</a> &nbsp; <a href=/control>Basic controls</a></p>";
  o << "<p><a href=/update>Firmware update</a> &nbsp; <a href=/wifi class=warn>Change WiFi</a></p>";
}

static void sendControlPage() {
  String zip = prefString(P_ZIP);
  HtmlOut o(200, "WFD Control");
  o << "<h2>WFD Clock Control</h2><form method=POST action=/control>";
  o << "<label>Brightness</label><input type=range name=bright min=1 max=16 value=" << disp.brightness << ">";
  o << "<label>Clock format</label><select name=clk24><option value=1" << (disp.clock24 ? " selected" : "")
    << ">24-hour</option><option value=0" << (disp.clock24 ? "" : " selected") << ">12-hour</option></select>";
  o << "<label>Weather ZIP</label><input name=zip value='" << esc(zip) << "' placeholder='e.g. 76708'>";
  o << "<label>Units</label><select name=units><option value=f" << (wx.units ? "" : " selected")
    << ">Fahrenheit</option><option value=c" << (wx.units ? " selected" : "") << ">Celsius</option></select>";
  o << "<label>Time zone</label><select name=tz>";
  const char *zn[][2] = {{"Eastern", "EST5EDT,M3.2.0,M11.1.0"}, {"Central", "CST6CDT,M3.2.0,M11.1.0"},
                         {"Mountain", "MST7MDT,M3.2.0,M11.1.0"}, {"Arizona", "MST7"},
                         {"Pacific", "PST8PDT,M3.2.0,M11.1.0"}, {"UTC", "UTC0"}};
  for (auto &z : zn) o << "<option value='" << z[1] << "'" << (net.tz == z[1] ? " selected" : "") << ">" << z[0] << "</option>";
  o << "</select>";
  o << "<div class=row><input type=checkbox name=micauto id=micauto " << (standalone.micAuto ? "checked" : "")
    << "><label for=micauto>Spectrum from the clock's mic when the app is off</label></div>";
  o << "<h3>Grid <small class=dim>when the app is off</small></h3>";
  o << "<div class=row><input type=checkbox name=gauto id=gauto " << (standalone.gaAuto ? "checked" : "")
    << "><label for=gauto>Play an animation</label></div>";
  o << "<label>Animation</label><select name=ganim>";
  for (int i = 1; i < grid::N_ANIM; i++) {
    String n = grid::NAMES[i];
    n.setCharAt(0, n[0] - 32);                       // "spin" -> "Spin"
    o << "<option value=" << i << (standalone.gaAnim == i ? " selected" : "") << ">" << n << "</option>";
  }
  o << "<option value=0" << (standalone.gaAnim == grid::CUSTOM ? " selected" : "") << (standalone.gaN ? "" : " disabled")
    << ">Drawn in the app (" << standalone.gaN << " frames)</option></select>";
  o << "<label>Speed <small>(frames per second)</small></label>"
       "<input type=range name=gfps min=2 max=25 value=" << standalone.gaFps << ">";
  o << "<div class=row><input type=checkbox name=gflag id=gflag " << (standalone.gaFlag ? "checked" : "")
    << "><label for=gflag>Light the border frame</label></div>";

  o << "<h3>Message line <small class=dim>when the app is off</small></h3>";
  o << "<div class=row><input type=checkbox name=omauto id=omauto " << (standalone.omAuto ? "checked" : "")
    << "><label for=omauto>Use these settings (off = temperature only)</label></div>";
  const char *modes[] = {"Temperature", "Text", "Date", "Rotate", "Forecast"};
  o << "<label>Show</label><select name=mmode>";
  for (int i = 0; i < N_SRC; i++)
    o << "<option value=" << i << (standalone.msgMode == i ? " selected" : "") << ">" << modes[i] << "</option>";
  o << "</select>";
  o << "<label>Text</label><input name=omtext maxlength=" << MSG_TEXT_MAX << " value='" << esc(standalone.msgText) << "'>";
  o << "<p class=dim style='margin:4px 0;font-size:13px'>Placeholders: {time} {time12} {date} {day} {temp}</p>";
  o << "<label>Rotate through</label><div class=row style=margin-top:4px>";
  const char *srcs[] = {"Temperature", "Text", "Date", "", "Forecast"};
  for (int i = 0; i < N_SRC; i++)
    if (i != MODE_ROTATE) o << "<input type=checkbox name=rot" << i << " id=rot" << i << ((standalone.msgRot >> i) & 1 ? " checked" : "")
                            << "><label for=rot" << i << ">" << srcs[i] << "</label>";
  o << "</div>";
  o << "<label>Seconds per item (rotate)</label><input type=number name=mdwell min=2 max=600 value=" << standalone.msgDwell << ">";
  o << "<label>Scroll speed <small>(ms per step, lower = faster)</small></label>"
       "<input type=range name=mscroll min=100 max=1000 step=50 value=" << standalone.msgScroll << ">";
  o << "<p class=dim style=font-size:13px>Now showing: <b>" << esc(standalone.msgShown.length() ? standalone.msgShown : String("—")) << "</b></p>";
  o << "<button type=submit>Apply</button></form>";
  o << "<p class=dim><a href=/>Dashboard</a> &middot; <a href=/status>Status</a> &middot; <a href=/update>Firmware update</a></p>";
}

static void sendJson(int code, JsonDocument &doc) {
  String body;
  serializeJson(doc, body);
  web.sendHeader("Cache-Control", "no-store");
  web.send(code, "application/json", body);
}

static void handleApiStatus() {
  JsonDocument doc;
  stateJson(doc["state"].to<JsonObject>());
  configJson(doc["config"].to<JsonObject>());
  sendJson(200, doc);
}

// POST /api/frames {"frames": [[r0..r6], ...], "fps": 8, "frame": false}: the app's drawing,
// over WiFi (the USB equivalent is GCLR / GADD / GCOMMIT).
static void handleApiFrames() {
  JsonDocument out;
  if (web.header("X-WFD") != "1") { out["error"] = "missing X-WFD header"; return sendJson(403, out); }
  if (!webAuthed()) return;
  JsonDocument in;
  if (deserializeJson(in, web.arg("plain")) || !in["frames"].is<JsonArrayConst>()) {
    out["error"] = "expected {\"frames\": [[7 row bytes], ...]}";
    return sendJson(400, out);
  }
  JsonArrayConst frames = in["frames"].as<JsonArrayConst>();
  int n = frames.size();
  if (n < 1 || n > GA_MAX) { out["error"] = "1.." + String(GA_MAX) + " frames"; return sendJson(400, out); }
  static uint8_t rows[GA_MAX * 7];
  int i = 0;
  for (JsonVariantConst f : frames) {
    if (!f.is<JsonArrayConst>() || f.size() > 7) { out["error"] = "each frame: up to 7 row bytes"; return sendJson(400, out); }
    for (int r = 0; r < 7; r++) rows[i * 7 + r] = r < (int)f.size() ? (uint8_t)f[r].as<int>() : 0;
    i++;
  }
  storeFrames(rows, n, in["fps"] | standalone.gaFps, (in["frame"] | false) ? 1 : 0);
  out["ok"] = true;
  out["frames"] = standalone.gaN;
  sendJson(200, out);
}

static void handleApiSet() {
  JsonDocument out;
  if (web.header("X-WFD") != "1") { out["error"] = "missing X-WFD header"; return sendJson(403, out); }
  if (!webAuthed()) return;
  JsonDocument in;
  if (deserializeJson(in, web.arg("plain")) || !in.is<JsonObject>()) {
    out["error"] = "expected a JSON object (send Content-Type: application/json)";
    return sendJson(400, out);
  }
  sendJson(applySettings(in.as<JsonObjectConst>(), out) ? 200 : 400, out);
}

// The /control form, translated to the same settings as the JSON API.
static void handleControlPost() {
  if (!webAuthed()) return;
  JsonDocument in, out;
  if (web.hasArg("bright")) in["brightness"] = web.arg("bright").toInt();
  if (web.hasArg("clk24")) in["clock_24h"] = web.arg("clk24").toInt() != 0;
  if (web.hasArg("units")) in["units"] = web.arg("units") == "c" ? "celsius" : "fahrenheit";
  if (web.hasArg("tz")) in["tz"] = web.arg("tz");
  String zip = web.arg("zip"); zip.trim();
  if (zip.length() && zip != prefString(P_ZIP)) in["location"] = zip;
  if (web.hasArg("omtext")) in["offline_text"] = web.arg("omtext");
  if (web.hasArg("mmode")) {
    in["standalone_msg_mode"] = MSG_MODE_NAMES[constrain(web.arg("mmode").toInt(), 0, N_SRC - 1)];
    JsonArray rot = in["standalone_rotate"].to<JsonArray>();
    for (int i = 0; i < N_SRC; i++) if (i != MODE_ROTATE && web.hasArg("rot" + String(i))) rot.add(MSG_MODE_NAMES[i]);
    in["standalone_dwell"] = web.arg("mdwell").toInt();
    in["standalone_scroll_ms"] = web.arg("mscroll").toInt();
  }
  if (web.hasArg("ganim")) {
    in["standalone_anim"] = grid::NAMES[constrain(web.arg("ganim").toInt(), 0, grid::N_ANIM - 1)];
    in["standalone_fps"] = web.arg("gfps").toInt();
    in["standalone_frame"] = web.hasArg("gflag");
  }
  in["grid_standalone"] = web.hasArg("gauto");
  in["mic_standalone"] = web.hasArg("micauto");
  in["msg_standalone"] = web.hasArg("omauto");
  if (!applySettings(in.as<JsonObjectConst>(), out)) {
    HtmlOut o(400, "WFD Control");
    o << "<h2 class=warn>Not saved</h2><ul>";
    for (JsonPairConst kv : out["error"].as<JsonObjectConst>())
      o << "<li>" << esc(kv.key().c_str()) << ": " << esc(kv.value().as<const char *>()) << "</li>";
    o << "</ul><p><a href=/control>Back</a></p>";
    return;
  }
  web.sendHeader("Location", "/control");
  web.send(303);
}

static const char *UPDATE_FORM =
    "<h2>Firmware Update</h2><form method=POST action=/update enctype=multipart/form-data>"
    "<input type=file name=fw accept=.bin><button type=submit>Upload &amp; Flash</button></form>"
    "<p class=dim>Upload a firmware .bin for this clock. It reboots when done. <a href=/>Back</a></p>";

void startWebRoutes() {
  static bool started = false;                   // the setup AP can open after the routes are up
  if (started) return;
  started = true;
  web.on("/", []() {                            // setup portal in AP mode, else the dashboard
    if (net.apMode) return sendSetupPage();
    if (!webAuthed()) return;                      // so the browser has the login for the API calls
    web.sendHeader("Content-Encoding", "gzip");
    web.send_P(200, "text/html", (const char *)DASHBOARD_GZ, DASHBOARD_GZ_LEN);
  });
  web.on("/status", []() { if (webAuthed()) sendStatusPage(); });   // names the WiFi and location
  web.on("/draw", HTTP_GET, []() {               // grid animator (the same page the PC app serves)
    if (!webAuthed()) return;
    web.sendHeader("Content-Encoding", "gzip");
    web.send_P(200, "text/html", (const char *)DRAW_GZ, DRAW_GZ_LEN);
  });
  web.on("/save", HTTP_POST, handleSave);
  web.on("/wifi", HTTP_GET, []() {                // change networks (also reachable in AP mode)
    if (!webAuthed()) return;
    sendSetupPage();
  });
  web.on("/rescan", []() {
    if (!webAuthed()) return;
    WiFi.scanDelete(); WiFi.scanNetworks(true);
    web.sendHeader("Location", "/wifi"); web.send(303);
  });
  web.on("/control", HTTP_GET, []() { if (webAuthed()) sendControlPage(); });
  web.on("/control", HTTP_POST, handleControlPost);
  web.on("/api/status", HTTP_GET, handleApiStatus);
  web.on("/api/set", HTTP_POST, handleApiSet);
  web.on("/api/frames", HTTP_POST, handleApiFrames);
  web.on("/api/frames", HTTP_GET, []() {         // the stored drawing, for the animator to start from
    JsonDocument out;
    JsonArray fr = out["frames"].to<JsonArray>();
    for (int i = 0; i < standalone.gaN; i++) {
      JsonArray row = fr.add<JsonArray>();
      for (int r = 0; r < 7; r++) row.add(standalone.gaFrames[i * 7 + r]);
    }
    out["fps"] = standalone.gaFps;
    out["frame"] = (bool)standalone.gaFlag;
    sendJson(200, out);
  });
  web.on("/api/preview", HTTP_POST, []() {       // the animator's live preview: show one frame now
    JsonDocument in, out;
    if (web.header("X-WFD") != "1") { out["error"] = "missing X-WFD header"; return sendJson(403, out); }
    if (!webAuthed()) return;
    if (deserializeJson(in, web.arg("plain")) || !in["rows"].is<JsonArrayConst>()) {
      out["error"] = "expected {\"rows\": [7 row bytes], \"frame\": 0/1}";
      return sendJson(400, out);
    }
    int r = 0;
    for (JsonVariantConst v : in["rows"].as<JsonArrayConst>()) if (r < 7) live.gridRows[r++] = v.as<int>() & 0x7F;
    while (r < 7) live.gridRows[r++] = 0;
    live.gridFrame = in["frame"].as<int>() ? 1 : 0;
    live.gridTime.mark();                               // shows like a live GRID for 1.5 s
    disp.dirty = true;
    out["ok"] = true;
    sendJson(200, out);
  });
  const char *hdrs[] = {"X-WFD"};
  web.collectHeaders(hdrs, 1);
  web.on("/update", HTTP_GET, []() { if (webAuthed()) sendPage(200, "WFD Update", UPDATE_FORM); });
  web.on("/update", HTTP_POST, []() {
    if (!webAuthed()) return;
    bool ok = otaOk && !Update.hasError();
    if (!ok) Update.printError(Serial);
    if (ok) sendPage(200, "WFD Update", "<h2>Update OK — rebooting…</h2>", "<meta http-equiv=refresh content='6;url=/'>");
    else sendPage(500, "WFD Update", "<h2 class=warn>Update FAILED</h2><p><a href=/update>Try again</a></p>");
    if (!ok) return;                    // keep running the current firmware
    web.client().flush();               // finish the reply and close cleanly, so the uploader
    web.client().stop();                //   doesn't see "connection reset" from the reboot
    delay(700);
    ESP.restart();
  }, handleUpdateUpload);
  web.onNotFound([]() {                          // captive portal: every URL is the setup page
    if (net.apMode) sendSetupPage();
    else web.send(404, "text/plain", "not found");
  });
  web.begin();
}
