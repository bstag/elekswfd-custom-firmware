// EleksWFD clock firmware (our own). Shows DS3231 time on the panel; time is set over USB.
// Serial @115200:
//   SET yy mo dd dow hh mi ss   set RTC (yy=2-digit year, dow 1=Mon..7=Sun), clears the lost-power flag
//   GET                         prints "yy mo dd dow hh mi ss osf"
#include <Arduino.h>
#include <Wire.h>
#include <Preferences.h>
#include <WiFi.h>
#include <WebServer.h>
#include <DNSServer.h>
#include <HTTPClient.h>
#include <WiFiClientSecure.h>
#include <Update.h>
#include <time.h>
#include "clock_panel.h"
#include "font14_table.h"

static Preferences prefs;

// WiFi / NTP state (declared early so serial command handlers can see them)
static WebServer web(80);
static DNSServer dns;
static bool apMode = false;
static bool ntpInit = false, ntpSynced = false;
static uint32_t lastNtp = 0;
static String g_tz;
static String g_lat, g_lon;           // weather location (empty = none)
static int g_wunit = 0;               // 0=F 1=C
static uint32_t lastWxFetch = 0;

static const int SDA_PIN = 21, SCL_PIN = 22;
static const uint8_t TM = 0x73, RTC = 0x68;
static uint8_t ram[WFD_RAM_BYTES];

static void tmCmd(uint8_t c) { Wire.beginTransmission(TM); Wire.write(c); Wire.endTransmission(); }
static void tmInit(uint8_t pwm) {
  const uint8_t s[] = {0x80, 0xA4, 0x98, 0x81, 0x83, (uint8_t)(0xB0 | (pwm & 0xF)), 0x88};
  for (uint8_t c : s) tmCmd(c);
}
static void tmFlush() {
  Wire.beginTransmission(TM);
  Wire.write(0x00);
  for (int i = 0; i < WFD_RAM_BYTES; i++) Wire.write(ram[i]);
  Wire.endTransmission();
}

static uint8_t bcd2dec(uint8_t v) { return (v >> 4) * 10 + (v & 0x0F); }
static uint8_t dec2bcd(uint8_t v) { return ((v / 10) << 4) | (v % 10); }

static bool rtcRead(uint8_t *o) {
  Wire.beginTransmission(RTC); Wire.write(0);
  if (Wire.endTransmission() != 0) return false;
  if (Wire.requestFrom(RTC, (uint8_t)7) != 7) return false;
  for (int i = 0; i < 7; i++) o[i] = Wire.read();
  return true;
}
static uint8_t rtcStatus() {
  Wire.beginTransmission(RTC); Wire.write(0x0F); Wire.endTransmission();
  Wire.requestFrom(RTC, (uint8_t)1); return Wire.read();
}
static void rtcSet(int yy, int mo, int dd, int dow, int hh, int mi, int ss) {
  Wire.beginTransmission(RTC); Wire.write(0);
  Wire.write(dec2bcd(ss)); Wire.write(dec2bcd(mi)); Wire.write(dec2bcd(hh));
  Wire.write(dow & 0x07); Wire.write(dec2bcd(dd)); Wire.write(dec2bcd(mo)); Wire.write(dec2bcd(yy));
  Wire.endTransmission();
  uint8_t st = rtcStatus();                       // clear OSF (lost-power flag)
  Wire.beginTransmission(RTC); Wire.write(0x0F); Wire.write(st & 0x7F); Wire.endTransmission();
}

struct Day { uint8_t byte, mask; };
static const Day DAYS[7] = {
  {DAY_MON_BYTE, DAY_MON_MASK}, {DAY_TUE_BYTE, DAY_TUE_MASK}, {DAY_WED_BYTE, DAY_WED_MASK},
  {DAY_THU_BYTE, DAY_THU_MASK}, {DAY_FRI_BYTE, DAY_FRI_MASK}, {DAY_SAT_BYTE, DAY_SAT_MASK},
  {DAY_SUN_BYTE, DAY_SUN_MASK},
};

static uint8_t curT[7] = {0};        // last RTC reading
static int specA = 0, specB = 0;      // spectrum column levels 0..12
static uint32_t specTime = 0;         // millis of last SPEC frame
static int barCpu = 0, barGpu = 0, barRam = 0;   // 0..100 percent
static uint32_t barsTime = 0;
static int wxCode = 0, wxTemp = 0, wxUnit = 0;   // code 0=none 1=sunny 2=cloud 3=rains; unit 0=F 1=C
static uint32_t wxTime = 0;

static uint8_t msgRam[12] = {0};      // raw glyphs for the 6 message chars (lo,hi x6 = bytes 0..11)
static uint32_t msgTime = 0;

// stored offline message animation (plays on the message chars when the app isn't driving them)
#define OM_MAX 100
static uint8_t omFrames[OM_MAX * 12];
static int omN = 0, omFps = 5, omAuto = 0, omIdx = 0;
static uint32_t omStep = 0;
static uint8_t omUp[OM_MAX * 12];
static int omUpN = 0;
static uint8_t gridRows[7] = {0};     // bit c (0..6) of row r = cell (r,c)
static uint8_t gridFrame = 0;
static uint32_t gridTime = 0;
static uint32_t appTime = 0;          // last time any serial command arrived (app alive)
static uint32_t iconMask = 0, iconTime = 0;   // top-row icons bitmask
static int logoOn = 0;
static uint32_t logoTime = 0;

// stored standalone animation (plays from flash when the app isn't driving the grid)
#define GA_MAX 80
static uint8_t gaFrames[GA_MAX * 7];
static int gaN = 0, gaFps = 8, gaFlag = 0, gaAuto = 0;
static uint8_t upBuf[GA_MAX * 7];
static int upN = 0;
static int gaIdx = 0;
static uint32_t gaStep = 0;

static void drawGrid(const uint8_t *rows, bool frame) {   // rows[r] bit c = cell (r,c)
  for (int r = 0; r < 7; r++)
    for (int c = 0; c < 7; c++)
      if (rows[r] & (1 << c)) {
        int i = r * 7 + c;
        ram[GRID_BYTE[i]] |= GRID_MASK[i];
      }
  if (frame)
    for (int i = 0; i < GRID_FRAME_N; i++) ram[GRID_FRAME_BYTE[i]] |= GRID_FRAME_MASK[i];
}

static void drawWeatherWord() {
  const uint8_t *w = wxCode == 1 ? WORD_SUNNY : wxCode == 2 ? WORD_CLOUD : wxCode == 3 ? WORD_RAINS : nullptr;
  if (w) { ram[w[0]] |= w[1]; if (w[2] || w[3]) ram[w[2]] |= w[3]; }
}

static void drawTemp() {
  int t = wxTemp, neg = t < 0; if (neg) t = -t;
  ram[ALNUM_LO[5]] = wxUnit ? CH_C : CH_F;         // rightmost char: F or C
  ram[ALNUM_LO[4]] = DIGIT_FONT[t % 10];           // units
  if (t >= 10) ram[ALNUM_LO[3]] = DIGIT_FONT[(t / 10) % 10];   // tens
  if (t >= 100) ram[ALNUM_LO[2]] = DIGIT_FONT[(t / 100) % 10]; // hundreds
  if (neg) ram[ALNUM_LO[t >= 10 ? 2 : 3]] = 0x04;  // minus sign = middle segment
}

static void drawBar(const uint8_t *BY, const uint8_t *MK, int pct,
                    uint8_t lb, uint8_t lm, uint8_t pb, uint8_t pm) {
  ram[lb] |= lm;                       // label (CPU/GPU/RAM text)
  ram[pb] |= pm;                       // percent sign
  int segs = (pct * 20 + 50) / 100;
  if (segs < 0) segs = 0; if (segs > 20) segs = 20;
  for (int i = 0; i < segs; i++) ram[BY[i]] |= MK[i];
}
static int specStyle = 4;             // 1=bars 2=top-down 3=center-out 4=peak dot 5=bars+peak
#define N_STYLES 5

static void setStyle(int v) {
  if (v < 1 || v > N_STYLES) return;
  if (v != specStyle) { specStyle = v; prefs.putInt("style", specStyle); }  // remember across power-off
}

static int clock24 = 1;               // 1 = 24-hour, 0 = 12-hour display
static int brightness = 6;            // 1..16 (TM1680 PWM)
static void setBrightness(int v) {
  if (v < 1) v = 1; if (v > 16) v = 16;
  brightness = v;
  tmCmd(0xB0 | ((v - 1) & 0x0F));
  prefs.putInt("bright", brightness);
}

static int peakVal[2] = {0, 0};
static uint32_t peakAt[2] = {0, 0};

static void drawCol(const uint8_t *BY, const uint8_t *MK, int level, int col) {
  if (level < 0) level = 0; if (level > 12) level = 12;
  int lo = 6 - level / 2, hi = 6 + (level + 1) / 2;   // for center style
  // peak-hold: rise instantly, fall ~1 level per 90 ms
  if (level >= peakVal[col]) { peakVal[col] = level; peakAt[col] = millis(); }
  else if (millis() - peakAt[col] > 90) { peakVal[col]--; peakAt[col] = millis(); }
  for (int i = 0; i < 12; i++) {                       // i=0 bottom .. i=11 top
    bool on = false;
    switch (specStyle) {
      case 1: on = i < level; break;                   // bars from bottom
      case 2: on = i >= 12 - level; break;             // bars from top
      case 3: on = (i >= lo && i < hi); break;         // grow from center
      case 4: on = (peakVal[col] > 0 && i == peakVal[col] - 1); break; // floating peak dot
      case 5: on = (i < level) || (peakVal[col] > 0 && i == peakVal[col] - 1); break; // bars + peak dot
    }
    if (on) ram[BY[i]] |= MK[i];
  }
}

static void draw() {
  memset(ram, 0, sizeof ram);
  const uint8_t *t = curT;
  int hh = bcd2dec(t[2] & 0x3F), mi = bcd2dec(t[1]), ss = bcd2dec(t[0] & 0x7F);
  bool blankTens = false;
  if (!clock24) {                       // 12-hour display
    hh %= 12; if (hh == 0) hh = 12;
    blankTens = (hh < 10);              // drop the leading zero (e.g. 1:23 not 01:23)
  }
  int d6[6] = {hh / 10, hh % 10, mi / 10, mi % 10, ss / 10, ss % 10};
  for (int i = 0; i < 6; i++)
    ram[DIGIT_BYTE[i]] = (i == 0 && blankTens) ? 0 : DIGIT_FONT[d6[i] % 10];
  for (int i = 0; i < 2; i++) ram[COLON_BYTE[i]] |= COLON_MASK[i];
  int dow = t[3] & 0x07;
  if (dow >= 1 && dow <= 7) ram[DAYS[dow - 1].byte] |= DAYS[dow - 1].mask;
  bool live = (millis() - specTime) < 600;          // clear columns if PC went quiet
  drawCol(SPEC_C1_BYTE, SPEC_C1_MASK, live ? specA : 0, 0);
  drawCol(SPEC_C2_BYTE, SPEC_C2_MASK, live ? specB : 0, 1);
  if (millis() - barsTime < 3000) {                 // bars when the PC is sending them
    drawBar(BAR_CPU_BYTE, BAR_CPU_MASK, barCpu, LBL_CPU_BYTE, LBL_CPU_MASK, PCT_CPU_BYTE, PCT_CPU_MASK);
    drawBar(BAR_GPU_BYTE, BAR_GPU_MASK, barGpu, LBL_GPU_BYTE, LBL_GPU_MASK, PCT_GPU_BYTE, PCT_GPU_MASK);
    drawBar(BAR_RAM_BYTE, BAR_RAM_MASK, barRam, LBL_RAM_BYTE, LBL_RAM_MASK, PCT_RAM_BYTE, PCT_RAM_MASK);
  }
  bool appOn = millis() - appTime < 2000;
  bool wxLive = millis() - wxTime < 1200000;   // weather fresh (PC every 60s, on-device every 15min)
  if (wxLive) drawWeatherWord();
  if (millis() - msgTime < 1500) {                  // live scrolling text from the app
    for (int i = 0; i < 12; i++) ram[i] = msgRam[i];
  } else if (wxLive) {
    drawTemp();                                     // live temperature
  } else if (!appOn && omAuto && omN > 0) {         // app off -> stored offline message
    for (int i = 0; i < 12; i++) ram[i] = omFrames[omIdx * 12 + i];
  }
  if (millis() - gridTime < 1500) drawGrid(gridRows, gridFrame);    // live grid from the app
  else if (!appOn && gaAuto && gaN > 0)                             // app off -> stored animation
    drawGrid(&gaFrames[gaIdx * 7], gaFlag);
  if (millis() - iconTime < 1500)                    // top-row status icons
    for (int i = 0; i < N_ICONS; i++)
      if (iconMask & (1UL << i)) {
        ram[ICON_B0[i]] |= ICON_M0[i];
        if (ICON_M1[i]) ram[ICON_B1[i]] |= ICON_M1[i];
      }
  if (logoOn && millis() - logoTime < 1500)          // ELEKSMAKER logo
    for (int i = 0; i < N_LOGO; i++) ram[LOGO_BYTE[i]] |= LOGO_MASK[i];
  tmFlush();
}

static String line;
static bool dirty = false;            // a command changed the display; loop() redraws once

// Parse up to n space-separated ints from s into out (unparsed slots keep their value).
static int parseInts(const String &s, int *out, int n) {
  int f = 0, i = 0, len = s.length();
  while (f < n && i < len) {
    while (i < len && s[i] == ' ') i++;
    int j = i; while (j < len && s[j] != ' ') j++;
    if (j > i) out[f++] = s.substring(i, j).toInt();
    i = j;
  }
  return f;
}

static void handle(const String &s) {
  appTime = millis();                 // any command means the app is alive
  int sp = s.indexOf(' ');
  String cmd = sp < 0 ? s : s.substring(0, sp);
  String arg = sp < 0 ? String() : s.substring(sp + 1);
  if (cmd == "SET") {
    int v[7];
    if (parseInts(arg, v, 7) == 7) { rtcSet(v[0], v[1], v[2], v[3], v[4], v[5], v[6]); Serial.println("OK"); }
    else Serial.println("ERR need 7 fields: yy mo dd dow hh mi ss");
  } else if (cmd == "SPEC") {
    int v[2] = {0, 0};
    parseInts(arg, v, 2);
    specA = constrain(v[0], 0, 12);
    specB = constrain(v[1], 0, 12);
    specTime = millis();
    dirty = true;
  } else if (cmd == "BARS") {
    int v[3] = {0, 0, 0};
    parseInts(arg, v, 3);
    barCpu = constrain(v[0], 0, 100);
    barGpu = constrain(v[1], 0, 100);
    barRam = constrain(v[2], 0, 100);
    barsTime = millis();
    dirty = true;
  } else if (cmd == "MSG") {
    int v[12] = {0};
    parseInts(arg, v, 12);
    for (int k = 0; k < 12; k++) msgRam[k] = (uint8_t)v[k];
    msgTime = millis();
    dirty = true;
  } else if (cmd == "GRID") {
    int v[8] = {0};
    parseInts(arg, v, 8);
    for (int k = 0; k < 7; k++) gridRows[k] = (uint8_t)(v[k] & 0x7F);
    gridFrame = v[7] ? 1 : 0;
    gridTime = millis();
    dirty = true;
  } else if (cmd == "GCLR") {
    upN = 0;
    Serial.println("OK");
  } else if (cmd == "GADD") {
    int v[7] = {0};
    parseInts(arg, v, 7);
    if (upN < GA_MAX) {
      for (int k = 0; k < 7; k++) upBuf[upN * 7 + k] = (uint8_t)(v[k] & 0x7F);
      upN++;
    }
  } else if (cmd == "GCOMMIT") {
    int v[2] = {8, 0};
    parseInts(arg, v, 2);
    gaN = upN; gaFps = v[0] < 1 ? 1 : v[0]; gaFlag = v[1] ? 1 : 0; gaAuto = 1; gaIdx = 0;
    memcpy(gaFrames, upBuf, gaN * 7);
    prefs.putBytes("ganim", gaFrames, gaN * 7);
    prefs.putInt("gn", gaN); prefs.putInt("gfps", gaFps);
    prefs.putInt("gflag", gaFlag); prefs.putInt("gauto", gaAuto);
    Serial.printf("OK stored %d frames\n", gaN);
  } else if (cmd == "GAUTO") {
    gaAuto = arg.toInt() ? 1 : 0;
    prefs.putInt("gauto", gaAuto);
    Serial.printf("OK auto %d\n", gaAuto);
  } else if (cmd == "OMCLR") {
    omUpN = 0;
    Serial.println("OK");
  } else if (cmd == "OMADD") {
    int v[12] = {0};
    parseInts(arg, v, 12);
    if (omUpN < OM_MAX) {
      for (int k = 0; k < 12; k++) omUp[omUpN * 12 + k] = (uint8_t)v[k];
      omUpN++;
    }
  } else if (cmd == "OMCOMMIT") {
    int v[1] = {5};
    parseInts(arg, v, 1);
    omN = omUpN; omFps = v[0] < 1 ? 1 : v[0];
    omAuto = 1; omIdx = 0;
    memcpy(omFrames, omUp, omN * 12);
    prefs.putBytes("om", omFrames, omN * 12);
    prefs.putInt("omn", omN); prefs.putInt("omfps", omFps); prefs.putInt("omauto", omAuto);
    Serial.printf("OK stored %d msg frames\n", omN);
  } else if (cmd == "OMAUTO") {
    omAuto = arg.toInt() ? 1 : 0;
    prefs.putInt("omauto", omAuto);
    Serial.printf("OK ommsg auto %d\n", omAuto);
  } else if (cmd == "WIFISCAN") {
    WiFi.disconnect();
    delay(200);
    int n = WiFi.scanNetworks();
    Serial.printf("scan: %d networks (2.4GHz only)\n", n);
    for (int i = 0; i < n; i++)
      Serial.printf("  '%s'  %d dBm %s\n", WiFi.SSID(i).c_str(), WiFi.RSSI(i),
                    WiFi.encryptionType(i) == WIFI_AUTH_OPEN ? "open" : "");
    WiFi.scanDelete();
  } else if (cmd == "WIFIINFO") {
    Serial.printf("wifi %s status=%d trying-ssid='%s' ip=%s rssi=%d ntp=%d\n",
                  apMode ? "AP" : (WiFi.status() == WL_CONNECTED ? "connected" : "connecting"),
                  (int)WiFi.status(), prefs.getString("ssid", "").c_str(),
                  WiFi.localIP().toString().c_str(), WiFi.RSSI(), ntpSynced);
  } else if (cmd == "WIFISET") {                // "WIFISET <ssid>\t<pass>" (tab-separated)
    int tab = arg.indexOf('\t');
    if (tab > 0) {
      prefs.putString("ssid", arg.substring(0, tab));
      prefs.putString("pass", arg.substring(tab + 1));
      Serial.println("OK wifi saved, restarting");
      delay(300); ESP.restart();
    } else Serial.println("ERR need ssid<TAB>pass");
  } else if (cmd == "GEO") {                    // "GEO <lat> <lon> <unit>"  set weather location
    int a = arg.indexOf(' '), b = arg.indexOf(' ', a + 1);
    if (a > 0 && b > 0) {
      g_lat = arg.substring(0, a);
      g_lon = arg.substring(a + 1, b);
      g_wunit = arg.substring(b + 1).toInt() ? 1 : 0;
      prefs.putString("lat", g_lat); prefs.putString("lon", g_lon); prefs.putInt("wunit", g_wunit);
      lastWxFetch = 0;                          // fetch now
      Serial.println("OK geo");
    } else Serial.println("ERR geo");
  } else if (cmd == "TZ") {
    g_tz = arg;
    prefs.putString("tz", g_tz);
    configTzTime(g_tz.c_str(), "pool.ntp.org", "time.nist.gov");
    Serial.println("OK tz");
  } else if (cmd == "WIFISSID") {               // fix just the network name, keep the password
    prefs.putString("ssid", arg);
    Serial.println("OK ssid updated, restarting");
    delay(300); ESP.restart();
  } else if (cmd == "WEBPASS") {                // web password for /control /update /wifi ("" = open)
    prefs.putString("webpass", arg);
    Serial.println(arg.length() ? "OK webpass set (user: admin)" : "OK webpass cleared");
  } else if (cmd == "WIFIRESET") {
    prefs.remove("ssid"); prefs.remove("pass");
    Serial.println("OK cleared, restarting");
    delay(300); ESP.restart();
  } else if (cmd == "ICON") {
    iconMask = (uint32_t)arg.toInt();
    iconTime = millis();
    dirty = true;
  } else if (cmd == "LOGO") {
    logoOn = arg.toInt() ? 1 : 0;
    logoTime = millis();
    dirty = true;
  } else if (cmd == "CLK24") {
    clock24 = arg.toInt() ? 1 : 0;
    prefs.putInt("clk24", clock24);
    dirty = true;
  } else if (cmd == "BRIGHT") {
    setBrightness(arg.toInt());
    Serial.printf("OK bright %d\n", brightness);
  } else if (cmd == "WX") {
    int v[3] = {0, 0, 0};
    parseInts(arg, v, 3);
    wxCode = constrain(v[0], 0, 3);
    wxTemp = constrain(v[1], -99, 199);
    wxUnit = v[2] ? 1 : 0;
    wxTime = millis();
    dirty = true;
  } else if (cmd == "STYLE") {
    int v = arg.toInt();
    if (v >= 1 && v <= N_STYLES) { setStyle(v); dirty = true; Serial.printf("OK style %d\n", specStyle); }
    else Serial.printf("ERR style 1..%d\n", N_STYLES);
  } else if (cmd == "GET") {
    uint8_t t[7];
    if (rtcRead(t))
      Serial.printf("%02d %02d %02d %d %02d %02d %02d osf=%d\n",
                    bcd2dec(t[6]), bcd2dec(t[5] & 0x1F), bcd2dec(t[4]), t[3] & 7,
                    bcd2dec(t[2] & 0x3F), bcd2dec(t[1]), bcd2dec(t[0] & 0x7F), rtcStatus() >> 7);
    else Serial.println("ERR rtc");
  } else {
    Serial.printf("ERR unknown command '%s'\n", cmd.c_str());
  }
}

// ============================= WiFi / NTP =============================
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
  if (apMode) return true;
  String pw = prefs.getString("webpass", "");
  if (pw.length() == 0 || web.authenticate("admin", pw.c_str())) return true;
  web.requestAuthentication();
  return false;
}

static String setupPage() {
  int n = WiFi.scanComplete();
  String opts;
  for (int i = 0; i < n; i++) opts += "<option>" + esc(WiFi.SSID(i)) + "</option>";
  return "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
         "<title>WFD WiFi Setup</title><style>body{font-family:sans-serif;background:#111;color:#eee;padding:22px}"
         "input,button{font-size:16px;padding:9px;width:100%;margin:6px 0;box-sizing:border-box;border-radius:6px;border:1px solid #444;background:#1d2228;color:#eee}"
         "button{background:#72d3ff;color:#06212c;font-weight:600;border:0}</style>"
         "<h2>WFD Clock WiFi</h2><form method=POST action=/save>"
         "<label>Network</label><input list=nets name=ssid placeholder='WiFi name' autocomplete=off>"
         "<datalist id=nets>" + opts + "</datalist>"
         "<label>Password</label><input name=pass type=password placeholder='WiFi password'>"
         "<button type=submit>Save &amp; Connect</button></form>"
         "<p style=color:#8b95a3>The clock restarts and joins your network. "
         "<a href=/rescan style=color:#72d3ff>Re-scan</a> to refresh the list.</p>";
}

static void handleSave() {
  if (!webAuthed()) return;
  if (web.arg("ssid").length() == 0) { web.sendHeader("Location", "/wifi"); web.send(303); return; }
  prefs.putString("ssid", web.arg("ssid"));
  prefs.putString("pass", web.arg("pass"));
  web.send(200, "text/html", "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
           "<body style='font-family:sans-serif;background:#111;color:#eee;padding:22px'>"
           "<h2>Saved — connecting…</h2><p>Rejoin your normal WiFi. The clock is restarting.</p>");
  delay(900);
  ESP.restart();
}

static String statusPage();            // fwd (defined after settings known)

// ---- on-device text rendering (14-seg font) + ZIP geocode, for the phone control page ----
static void glyph14(char c, uint8_t *lo, uint8_t *hi) {
  if (c >= 'a' && c <= 'z') c -= 32;
  if (c >= 32 && c < 128) { *lo = FONT14_LO[c - 32]; *hi = FONT14_HI[c - 32]; }
  else { *lo = 0; *hi = 0; }
}
static void render6(const String &s, int start, uint8_t *out) {   // out[12]
  for (int i = 0; i < 6; i++) {
    int idx = start + i;
    char c = (idx >= 0 && idx < (int)s.length()) ? s[idx] : ' ';
    glyph14(c, &out[i * 2], &out[i * 2 + 1]);
  }
}
static void setOfflineText(String t) {
  t.trim();
  if (t.length() == 0) t = " ";
  if (t.length() > OM_MAX - 3) t = t.substring(0, OM_MAX - 3);   // +3 gap = one frame per position
  int n;
  if (t.length() <= 6) {
    n = 1;
    render6(t, 0, omFrames);
  } else {
    String d = t + "   ";
    n = d.length();
    String dd = d + d;
    for (int p = 0; p < n; p++) render6(dd, p, &omFrames[p * 12]);
  }
  omN = n; omFps = (n == 1 ? 1 : 3); omAuto = 1; omIdx = 0;
  prefs.putBytes("om", omFrames, omN * 12);
  prefs.putInt("omn", omN); prefs.putInt("omfps", omFps); prefs.putInt("omauto", omAuto);
  prefs.putString("omtext", t);
}
static bool setWeatherZip(const String &zip) {
  if (WiFi.status() != WL_CONNECTED) return false;
  WiFiClient cli;
  HTTPClient http;
  http.setTimeout(6000);
  if (!http.begin(cli, "http://api.zippopotam.us/us/" + zip)) return false;
  bool ok = false;
  if (http.GET() == 200) {
    String p = http.getString();
    int la = p.indexOf("\"latitude\": \"");
    int lo = p.indexOf("\"longitude\": \"");
    if (la > 0 && lo > 0) {
      g_lat = p.substring(la + 13, p.indexOf('"', la + 13));
      g_lon = p.substring(lo + 14, p.indexOf('"', lo + 14));
      prefs.putString("lat", g_lat); prefs.putString("lon", g_lon); prefs.putString("zip", zip);
      lastWxFetch = 0;
      ok = true;
    }
  }
  http.end();
  return ok;
}

static String controlPage() {
  String zip = prefs.getString("zip", "");
  String omt = prefs.getString("omtext", "");
  String s = "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'><title>WFD Control</title>"
    "<style>body{font-family:sans-serif;background:#0e1013;color:#e6e9ee;padding:18px;max-width:480px;margin:auto}"
    "label{display:block;color:#8b95a3;margin-top:14px;font-size:13px}"
    "input,select{width:100%;font-size:16px;padding:9px;margin-top:4px;box-sizing:border-box;border-radius:6px;border:1px solid #2a3038;background:#1d2228;color:#eee}"
    "button{margin-top:18px;width:100%;font-size:16px;padding:12px;border:0;border-radius:6px;background:#72d3ff;color:#06212c;font-weight:600}"
    ".row{display:flex;gap:10px;align-items:center;margin-top:14px}.row input{width:auto}.row label{margin:0}</style>"
    "<h2>WFD Clock Control</h2><form method=POST action=/control>";
  s += "<label>Brightness</label><input type=range name=bright min=1 max=16 value=" + String(brightness) + ">";
  s += "<label>Clock format</label><select name=clk24><option value=1" + String(clock24 ? " selected" : "") +
       ">24-hour</option><option value=0" + String(clock24 ? "" : " selected") + ">12-hour</option></select>";
  s += "<label>Weather ZIP</label><input name=zip value='" + esc(zip) + "' placeholder='e.g. 76708'>";
  s += "<label>Units</label><select name=units><option value=f" + String(g_wunit ? "" : " selected") +
       ">Fahrenheit</option><option value=c" + String(g_wunit ? " selected" : "") + ">Celsius</option></select>";
  s += "<label>Time zone</label><select name=tz>";
  const char *zn[][2] = {{"Eastern", "EST5EDT,M3.2.0,M11.1.0"}, {"Central", "CST6CDT,M3.2.0,M11.1.0"},
                         {"Mountain", "MST7MDT,M3.2.0,M11.1.0"}, {"Arizona", "MST7"},
                         {"Pacific", "PST8PDT,M3.2.0,M11.1.0"}, {"UTC", "UTC0"}};
  for (auto &z : zn) s += "<option value='" + String(z[1]) + "'" + (g_tz == z[1] ? " selected" : "") + ">" + z[0] + "</option>";
  s += "</select>";
  s += "<label>Offline message (when the app is off)</label><input name=omtext maxlength=" + String(OM_MAX - 3) + " value='" + esc(omt) + "'>";
  s += "<div class=row><input type=checkbox name=gauto " + String(gaAuto ? "checked" : "") +
       "><label>Play stored grid animation when app is off</label></div>";
  s += "<div class=row><input type=checkbox name=omauto " + String(omAuto ? "checked" : "") +
       "><label>Show offline message when app is off</label></div>";
  s += "<button type=submit>Apply</button></form>";
  s += "<p style=color:#8b95a3><a href=/ style=color:#72d3ff>Status</a> &middot; "
       "<a href=/update style=color:#72d3ff>Firmware update</a></p>";
  return s;
}
static void handleControlPost() {
  if (!webAuthed()) return;
  if (web.hasArg("bright") && web.arg("bright").toInt() != brightness) setBrightness(web.arg("bright").toInt());
  if (web.hasArg("clk24")) { clock24 = web.arg("clk24").toInt() ? 1 : 0; prefs.putInt("clk24", clock24); dirty = true; }
  if (web.hasArg("units")) {
    int u = web.arg("units") == "c" ? 1 : 0;
    if (u != g_wunit) { g_wunit = u; prefs.putInt("wunit", g_wunit); lastWxFetch = 0; }   // refetch in new units
  }
  if (web.hasArg("tz") && web.arg("tz") != g_tz) {
    g_tz = web.arg("tz"); prefs.putString("tz", g_tz);
    configTzTime(g_tz.c_str(), "pool.ntp.org", "time.nist.gov");
  }
  String zip = web.arg("zip"); zip.trim();
  if (zip.length() >= 5 && zip != prefs.getString("zip", "")) setWeatherZip(zip);
  if (web.hasArg("omtext")) {
    String t = web.arg("omtext"); t.trim();
    if (t != prefs.getString("omtext", "")) setOfflineText(t);
  }
  gaAuto = web.hasArg("gauto") ? 1 : 0; prefs.putInt("gauto", gaAuto);
  omAuto = web.hasArg("omauto") ? 1 : 0; prefs.putInt("omauto", omAuto);
  web.sendHeader("Location", "/control");
  web.send(303);
}

static bool otaOk = false;             // set by the upload handler, read by the final POST handler
static void handleUpdateUpload() {
  HTTPUpload &up = web.upload();
  if (up.status == UPLOAD_FILE_START) {
    otaOk = false;
    String pw = prefs.getString("webpass", "");
    if (pw.length() && !web.authenticate("admin", pw.c_str())) return;   // POST handler sends the 401
    otaOk = Update.begin(UPDATE_SIZE_UNKNOWN);
  } else if (up.status == UPLOAD_FILE_WRITE) {
    if (otaOk && Update.write(up.buf, up.currentSize) != up.currentSize) otaOk = false;
  } else if (up.status == UPLOAD_FILE_END) {
    if (otaOk) otaOk = Update.end(true);
  } else if (up.status == UPLOAD_FILE_ABORTED) {
    if (otaOk) Update.abort();
    otaOk = false;
  }
  if (!otaOk && up.status != UPLOAD_FILE_START && Update.isRunning()) Update.abort();
}

static const char *UPDATE_FORM =
    "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'><title>WFD Update</title>"
    "<style>body{font-family:sans-serif;background:#0e1013;color:#e6e9ee;padding:22px}"
    "input,button{font-size:16px;margin:8px 0}button{background:#72d3ff;color:#06212c;border:0;padding:10px 16px;border-radius:6px;font-weight:600}</style>"
    "<h2>Firmware Update</h2><form method=POST action=/update enctype=multipart/form-data>"
    "<input type=file name=fw accept=.bin><br><button type=submit>Upload &amp; Flash</button></form>"
    "<p style=color:#8b95a3>Upload a firmware .bin for this clock. It reboots when done. <a href=/ style=color:#72d3ff>Back</a></p>";

static void startWebRoutes() {
  web.on("/", []() { web.send(200, "text/html", apMode ? setupPage() : statusPage()); });
  web.on("/save", HTTP_POST, handleSave);
  web.on("/wifi", HTTP_GET, []() {                // change networks (also reachable in AP mode)
    if (!webAuthed()) return;
    if (WiFi.scanComplete() == WIFI_SCAN_FAILED) WiFi.scanNetworks(true);
    web.send(200, "text/html", setupPage());
  });
  web.on("/rescan", []() {
    WiFi.scanDelete(); WiFi.scanNetworks(true);
    web.sendHeader("Location", "/wifi"); web.send(303);
  });
  web.on("/control", HTTP_GET, []() { if (webAuthed()) web.send(200, "text/html", controlPage()); });
  web.on("/control", HTTP_POST, handleControlPost);
  web.on("/update", HTTP_GET, []() { if (webAuthed()) web.send(200, "text/html", UPDATE_FORM); });
  web.on("/update", HTTP_POST, []() {
    if (!webAuthed()) return;
    bool ok = otaOk && !Update.hasError();
    if (!ok) Update.printError(Serial);
    web.send(ok ? 200 : 500, "text/html", ok
      ? "<!doctype html><meta http-equiv=refresh content='6;url=/'><body style='font-family:sans-serif;background:#0e1013;color:#e6e9ee;padding:22px'><h2>Update OK — rebooting…</h2>"
      : "<body style='font-family:sans-serif;background:#0e1013;color:#ff5d5d;padding:22px'><h2>Update FAILED</h2>");
    if (!ok) return;                    // keep running the current firmware
    delay(700);
    ESP.restart();
  }, handleUpdateUpload);
  web.onNotFound([]() { web.send(200, "text/html", apMode ? setupPage() : statusPage()); });
  web.begin();
}

static void startAP() {
  apMode = true;
  WiFi.mode(WIFI_AP_STA);
  WiFi.scanNetworks(true);
  WiFi.softAP("WFD-Setup");
  dns.start(53, "*", WiFi.softAPIP());
  startWebRoutes();
}

static void wifiBegin() {
  g_tz = prefs.getString("tz", "CST6CDT,M3.2.0,M11.1.0");   // US Central w/ DST (default)
  g_lat = prefs.getString("lat", ""); g_lon = prefs.getString("lon", ""); g_wunit = prefs.getInt("wunit", 0);
  String ssid = prefs.getString("ssid", "");
  if (ssid.length()) {
    WiFi.mode(WIFI_STA);
    WiFi.setHostname("wfd-clock");
    WiFi.setSleep(false);               // keep radio responsive / stable
    WiFi.setAutoReconnect(true);
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

static void netWeatherFetch() {       // fetch Open-Meteo directly over HTTPS
  if (WiFi.status() != WL_CONNECTED || g_lat.length() == 0) return;
  WiFiClientSecure cli;
  cli.setInsecure();
  HTTPClient http;
  http.setTimeout(6000);
  String url = "https://api.open-meteo.com/v1/forecast?latitude=" + g_lat + "&longitude=" + g_lon +
               "&current=temperature_2m,weather_code&temperature_unit=" + (g_wunit ? "celsius" : "fahrenheit");
  if (!http.begin(cli, url)) return;
  int rc = http.GET();
  if (rc == 200) {
    String p = http.getString();
    int cur = p.indexOf("\"current\":{");       // skip the current_units block
    int ti = p.indexOf("temperature_2m\":", cur < 0 ? 0 : cur);
    int ci = p.indexOf("weather_code\":", cur < 0 ? 0 : cur);
    if (ti > 0 && ci > 0) {
      int temp = (int)lroundf(p.substring(ti + 16).toFloat());
      int code = p.substring(ci + 14).toInt();
      wxCode = code <= 1 ? 1 : (code == 2 || code == 3 || code == 45 || code == 48) ? 2 : 3;
      wxTemp = temp;
      wxUnit = g_wunit;
      wxTime = millis();
    }
  }
  http.end();
}

static void wifiLoop() {
  if (apMode) {
    dns.processNextRequest();
    web.handleClient();
    return;
  }
  if (WiFi.status() == WL_CONNECTED) {
    if (!ntpInit) {
      configTzTime(g_tz.c_str(), "pool.ntp.org", "time.nist.gov", "time.google.com");
      startWebRoutes();
      ntpInit = true;
    }
    web.handleClient();
    if (millis() - lastNtp > (ntpSynced ? 3600000UL : 4000UL)) {   // re-sync hourly once locked
      lastNtp = millis();
      ntpToRtc();
    }
    if (ntpSynced && (lastWxFetch == 0 || millis() - lastWxFetch > 900000UL)) {  // weather every 15 min
      lastWxFetch = millis();
      netWeatherFetch();
    }
  } else {
    static uint32_t lastTry = 0;        // not connected -> keep trying to reconnect
    if (millis() - lastTry > 8000) {
      lastTry = millis();
      WiFi.reconnect();
    }
  }
}

static String statusPage() {
  uint8_t t[7];
  char tm[32] = "—";
  if (rtcRead(t))
    snprintf(tm, sizeof tm, "20%02d-%02d-%02d %02d:%02d:%02d",
             bcd2dec(t[6]), bcd2dec(t[5] & 0x1F), bcd2dec(t[4]),
             bcd2dec(t[2] & 0x3F), bcd2dec(t[1]), bcd2dec(t[0] & 0x7F));
  String s = "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>"
             "<title>WFD Clock</title><style>body{font-family:sans-serif;background:#0e1013;color:#e6e9ee;padding:22px}"
             "b{color:#72d3ff}</style><h2>WFD Clock <small style=color:#8b95a3>fw: OTA-OK</small></h2>";
  s += "<p>Time: <b>" + String(tm) + "</b></p>";
  s += "<p>NTP: <b>" + String(ntpSynced ? "synced" : "waiting…") + "</b> &nbsp; WiFi: <b>" + esc(WiFi.SSID()) +
       "</b> (" + WiFi.localIP().toString() + ", " + String(WiFi.RSSI()) + " dBm)</p>";
  const char *cond = wxCode == 1 ? "Sunny" : wxCode == 2 ? "Cloudy" : wxCode == 3 ? "Rain" : "—";
  bool wxfresh = millis() - wxTime < 1200000;
  s += "<p>Weather: <b>" + (wxfresh ? String(wxTemp) + (wxUnit ? "C " : "F ") + cond : String("—")) +
       "</b> &nbsp; loc " + (g_lat.length() ? g_lat + "," + g_lon : String("unset")) + "</p>";
  s += "<p><a href=/control style='color:#72d3ff;font-weight:600'>⚙ Control panel</a></p>";
  s += "<p><a href=/update style='color:#72d3ff'>Firmware update</a> &nbsp; "
       "<a href=/wifi style='color:#ff5d5d'>Change WiFi</a></p>";
  return s;
}

void setup() {
  Serial.begin(115200);
  pinMode(25, INPUT);
  pinMode(26, INPUT);
  pinMode(27, INPUT);
  Wire.begin(SDA_PIN, SCL_PIN, 100000);
  delay(100);
  tmInit(6);
  prefs.begin("wfd", false);
  specStyle = prefs.getInt("style", 4);   // restore saved style, default = peak dot
  brightness = prefs.getInt("bright", 6);
  clock24 = prefs.getInt("clk24", 1);
  tmCmd(0xB0 | ((brightness - 1) & 0x0F));
  gaN = constrain(prefs.getInt("gn", 0), 0, GA_MAX);   // restore stored standalone animation
  gaFps = prefs.getInt("gfps", 8);
  gaFlag = prefs.getInt("gflag", 0);
  gaAuto = prefs.getInt("gauto", 0);
  if (gaN > 0 && prefs.getBytes("ganim", gaFrames, gaN * 7) != (size_t)gaN * 7) gaN = 0;
  omN = constrain(prefs.getInt("omn", 0), 0, OM_MAX);  // restore stored offline message
  omFps = prefs.getInt("omfps", 5);
  omAuto = prefs.getInt("omauto", 0);
  if (omN > 0 && prefs.getBytes("om", omFrames, omN * 12) != (size_t)omN * 12) omN = 0;
  wifiBegin();
  Serial.printf("wfd-clock ready, style %d, bright %d, grid %d auto %d, ommsg %d auto %d, wifi %s\n",
                specStyle, brightness, gaN, gaAuto, omN, omAuto, apMode ? "setup-AP" : "connecting");
}

static void checkButtons() {
  static uint8_t prev[3] = {1, 1, 1};
  static uint32_t lastPress = 0;
  const int pins[3] = {25, 26, 27};
  for (int i = 0; i < 3; i++) {
    uint8_t v = digitalRead(pins[i]);
    if (prev[i] == 1 && v == 0 && millis() - lastPress > 200) {   // falling edge, debounced
      lastPress = millis();
      if (i == 0) setStyle(specStyle <= 1 ? N_STYLES : specStyle - 1);   // left: prev
      else setStyle(specStyle >= N_STYLES ? 1 : specStyle + 1);          // middle/right: next
      Serial.printf("button %d -> style %d\n", i, specStyle);
      draw();
    }
    prev[i] = v;
  }
}

void loop() {
  static uint32_t last = 0;
  while (Serial.available()) {
    char c = Serial.read();
    if (c == '\n' || c == '\r') { if (line.length()) { handle(line); line = ""; } }
    else if (line.length() < 200) line += c;
  }
  wifiLoop();
  checkButtons();
  // advance stored animations only when the app is off
  bool redraw = false;
  bool appOff = millis() - appTime > 2000;
  if (appOff && gaAuto && gaN > 0 &&
      millis() - gaStep > (uint32_t)(1000 / (gaFps < 1 ? 1 : gaFps))) {
    gaStep = millis();
    gaIdx = (gaIdx + 1) % gaN;
    redraw = true;
  }
  if (appOff && omAuto && omN > 0 &&
      millis() - omStep > (uint32_t)(1000 / (omFps < 1 ? 1 : omFps))) {
    omStep = millis();
    omIdx = (omIdx + 1) % omN;
    redraw = true;
  }
  if (millis() - last >= 250) {
    last = millis();
    if (rtcRead(curT)) redraw = true;
  }
  if (redraw || dirty) {               // one I2C flush per pass, however many commands arrived
    dirty = false;
    draw();
  }
}
