// EleksWFD clock firmware (our own). Shows DS3231 time on the panel; time is set over USB.
// Serial @115200:
//   SET yy mo dd dow hh mi ss   set RTC (yy=2-digit year, dow 1=Mon..7=Sun), clears the lost-power flag
//   GET                         prints "yy mo dd dow hh mi ss osf"
#include <Arduino.h>
#include <Wire.h>
#include <Preferences.h>
#include <WiFi.h>
#include <WiFiUdp.h>
#include <ESPmDNS.h>
#include <WebServer.h>
#include <DNSServer.h>
#include <HTTPClient.h>
#include <WiFiClientSecure.h>
#include <Update.h>
#include <time.h>
#include <ArduinoJson.h>
#include "clock_panel.h"
#include "font14_table.h"
#include "grid_engine.h"
#include "mic_dsp.h"
#include "web_pages.h"

static Preferences prefs;

// What the PC app checks before talking to this firmware. Bump WFD_PROTOCOL whenever commands,
// settings or the JSON API change in a way the app must know about (app.py PROTOCOL).
//   1 serial display commands   2 settings over USB (CFG/CFGSET) + JSON API   3 WiFi link (UDP, BYE, /api/frames)
#define WFD_PROTOCOL 3
static const char *FW_BUILD = "wfd-clock " __DATE__ " " __TIME__;

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

// on-device message line (runs when the app isn't driving the message chars)
enum { SRC_TEMP = 0, SRC_TEXT = 1, SRC_DATE = 2, N_SRC = 3, MODE_ROTATE = 3 };
#define MSG_TEXT_MAX 96
static int omAuto = 0;                // 1 = run this engine when the app is off, 0 = just the temperature
static int msgMode = SRC_TEXT;        // SRC_TEMP / SRC_TEXT / SRC_DATE / MODE_ROTATE
static int msgRot = 0b111;            // rotate: bit per SRC_*
static int msgDwell = 8;              // rotate: seconds per source
static int msgScroll = 350;           // ms per scroll step
static String msgText = "WFD CLOCK";  // text source; {time} {time12} {date} {day} {temp} expand live
static uint8_t engRam[12] = {0};      // engine output glyphs
static bool engTemp = true;           // engine says: show the temperature (drawTemp) instead of engRam
static String msgShown;               // text currently on the message line (MSGINFO, phone page)
static void setMsgText(String t);
static void setMsgCfg(int mode, int rot, int dwell, int scroll);
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
static int gaAnim = grid::SPIN;       // what plays when the app is off: grid::CUSTOM = gaFrames
static grid::Engine gEng(esp_random);
static uint8_t gEngRows[7] = {0};     // current built-in animation frame (audio/mouth read micBands)
static bool gridReady() { return gaAnim != grid::CUSTOM || gaN > 0; }
static void setGridAnim(int anim, int fps, int flag);
static const int MIC_PIN = 32;        // microphone: GPIO32 / ADC1_CH4, behind an op-amp
static int micAuto = 1;               // spectrum columns from the mic when the app is off
static mic::Dsp micDsp;               // runs in micTask on core 0
static volatile int micBass = 0, micTreble = 0, micLevel = 0;   // latest results
static double micBands[mic::NBANDS] = {0};                      // grid audio/mouth input
static volatile uint32_t micFrameAt = 0;                        // millis of the last frame
static volatile bool micPause = false;                          // MIC/MICLOG use the ADC
static volatile uint32_t micTraceUntil = 0;                     // MICTRACE: print every frame
static void micCapture(int n, int rate);
static void micLog(int secs);
static void serialCfg(const String &json);   // CFG / CFGSET: the settings layer over USB
static void storeFrames(const uint8_t *rows, int n, int fps, int flag);
// The PC app over WiFi: UDP packets "WFD1 <key>\n<command>\n<command>..." (key = web password,
// if set). Only live-display commands are accepted; settings go through the HTTP API.
static const uint16_t UDP_PORT = 4210;
static const char *HOSTNAME = "wfd-clock";      // -> wfd-clock.local (mDNS)
static WiFiUDP udp;
static bool netUp = false;
static uint32_t udpPackets = 0, udpRejected = 0;
static String webPass;                          // cached "webpass" (checked 30x a second)

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
  bool live = (millis() - specTime) < 600;          // PC audio (clears if the PC went quiet)
  bool micLive = !live && micAuto && millis() - appTime > 2000 && millis() - micFrameAt < 500;
  drawCol(SPEC_C1_BYTE, SPEC_C1_MASK, live ? specA : micLive ? micBass : 0, 0);      // mic: bass
  drawCol(SPEC_C2_BYTE, SPEC_C2_MASK, live ? specB : micLive ? micTreble : 0, 1);    // mic: treble
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
  } else if (!appOn && omAuto && !engTemp) {        // app off -> on-device message engine
    for (int i = 0; i < 12; i++) ram[i] = engRam[i];
  } else if (wxLive) {
    drawTemp();                                     // live temperature
  }
  if (millis() - gridTime < 1500) drawGrid(gridRows, gridFrame);    // live grid from the app
  else if (!appOn && gaAuto && gridReady())                         // app off -> own animation
    drawGrid(gaAnim == grid::CUSTOM ? &gaFrames[gaIdx * 7] : gEngRows, gaFlag);
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
    storeFrames(upBuf, upN, v[0], v[1]);
    Serial.printf("OK stored %d frames\n", gaN);
  } else if (cmd == "GANIM") {                  // "GANIM <name> [fps] [frame]"  built-in animation
    int sp2 = arg.indexOf(' ');
    String name = sp2 < 0 ? arg : arg.substring(0, sp2);
    int v[2] = {gaFps, gaFlag};
    if (sp2 > 0) parseInts(arg.substring(sp2 + 1), v, 2);
    int a = grid::animFromName(name.c_str());
    if (a < 0) Serial.printf("ERR unknown animation '%s'\n", name.c_str());
    else { setGridAnim(a, v[0], v[1]); Serial.printf("OK ganim %s %d %d\n", grid::NAMES[gaAnim], gaFps, gaFlag); }
  } else if (cmd == "GINFO") {
    const uint8_t *r = gaAnim == grid::CUSTOM ? &gaFrames[gaIdx * 7] : gEngRows;
    Serial.printf("grid auto=%d anim=%s fps=%d frame=%d stored=%d rows=%d %d %d %d %d %d %d\n", gaAuto,
                  grid::NAMES[gaAnim], gaFps, gaFlag, gaN, r[0], r[1], r[2], r[3], r[4], r[5], r[6]);
  } else if (cmd == "GAUTO") {
    gaAuto = arg.toInt() ? 1 : 0;
    prefs.putInt("gauto", gaAuto);
    Serial.printf("OK auto %d\n", gaAuto);
  } else if (cmd == "OMTEXT") {                 // "OMTEXT <text>"  message text, placeholders allowed
    setMsgText(arg);
    Serial.printf("OK omtext '%s'\n", msgText.c_str());
  } else if (cmd == "MSGCFG") {                 // "MSGCFG <mode> <rotmask> <dwell s> <scroll ms>"
    int v[4] = {msgMode, msgRot, msgDwell, msgScroll};
    parseInts(arg, v, 4);
    setMsgCfg(v[0], v[1], v[2], v[3]);
    Serial.printf("OK msgcfg %d %d %d %d\n", msgMode, msgRot, msgDwell, msgScroll);
  } else if (cmd == "MSGINFO") {                // what the engine is showing right now (for testing)
    Serial.printf("msg auto=%d mode=%d rot=%d dwell=%d scroll=%d src=%s text='%s'\n", omAuto, msgMode,
                  msgRot, msgDwell, msgScroll, engTemp ? "temp" : "glyphs", msgShown.c_str());
  } else if (cmd == "MIC") {                    // mic diagnostics: one fast capture + stats
    micPause = true; delay(40);                 // let micTask finish its frame
    if (arg.length()) {                         // "MIC <0..3>": ADC attenuation 0/2.5/6/11 dB
      const adc_attenuation_t at[] = {ADC_0db, ADC_2_5db, ADC_6db, ADC_11db};
      analogSetPinAttenuation(MIC_PIN, at[constrain(arg.toInt(), 0, 3)]);
    }
    micCapture(2048, 10000);
    analogSetPinAttenuation(MIC_PIN, ADC_11db);
    micPause = false;
  } else if (cmd == "MICTRACE") {               // "MICTRACE <s>": per frame "T ms rms low high level% bass treble"
    micTraceUntil = millis() + 1000UL * constrain(arg.toInt(), 1, 120);
    appTime = millis() - 3000;                  // this command shouldn't count as the app being on
  } else if (cmd == "MICLOG") {                 // "MICLOG <s>": level bar every 100 ms
    micPause = true; delay(40);
    micLog(constrain(arg.toInt(), 1, 60));
    micPause = false;
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
  } else if (cmd == "BYE") {                    // the app is quitting: take over now, not in 2 s
    appTime = millis() - 10000;
    specTime = barsTime = msgTime = gridTime = iconTime = logoTime = 0;
    Serial.println("OK bye");
  } else if (cmd == "WEBPASS") {                // web password for /control /update /wifi ("" = open)
    prefs.putString("webpass", arg);
    webPass = arg;
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
  } else if (cmd == "CFG") {                    // print every setting: "CFG {json}"
    serialCfg("");
  } else if (cmd == "CFGSET") {                 // "CFGSET {json}" -> "CFGOK {settings}" or "CFGERR {errors}"
    serialCfg(arg);
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
  if (webPass.length() == 0 || web.authenticate("admin", webPass.c_str())) return true;
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

static String page(const char *title, const String &body, const char *head = "") {
  return String("<!doctype html><meta charset=utf-8>"
                "<meta name=viewport content='width=device-width,initial-scale=1'>") +
         head + "<title>" + title + "</title><style>" + PAGE_CSS + "</style>" + body;
}

static const char *signalBars(int rssi) {
  return rssi > -55 ? "▂▄▆█" : rssi > -67 ? "▂▄▆" : rssi > -78 ? "▂▄" : "▂";
}

// WiFi picker: tap a scanned network (strongest first) or type one under "Other".
// Works without JS; the inline script only focuses the right field and hides the
// password box for open networks.
static String setupPage() {
  int n = WiFi.scanComplete();
  if (n == WIFI_SCAN_FAILED) { WiFi.scanNetworks(true); n = WIFI_SCAN_RUNNING; }
  if (n == WIFI_SCAN_RUNNING)
    return page("WFD WiFi Setup", "<h2>WFD Clock WiFi</h2><p>Scanning for networks…</p>",
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

  String current = prefs.getString("ssid", "");
  String list;
  for (int k = 0; k < m; k++) {
    int i = idx[k];
    bool open = WiFi.encryptionType(i) == WIFI_AUTH_OPEN;
    String s = esc(WiFi.SSID(i));
    list += "<label class=net><input type=radio name=ssid value='" + s + "' data-open=" + (open ? "1" : "0") +
            (WiFi.SSID(i) == current ? " checked" : "") + "><span>" + s + "</span>" +
            (open ? "" : "🔒") + "<b class=sig>" + signalBars(WiFi.RSSI(i)) + "</b></label>";
  }
  if (m == 0) list = "<p class=dim>No networks found.</p>";
  list += "<label class=net><input type=radio name=ssid value='' id=oth data-open=0><span>Other network…</span></label>"
          "<input name=other id=othname placeholder='Network name (hidden networks)' autocomplete=off>";

  return page("WFD WiFi Setup",
         "<h2>WFD Clock WiFi</h2><form method=POST action=/save>"
         "<label>Choose a network <a href=/rescan style=float:right>↻ Scan again</a></label>" + list +
         "<div id=pw><label>Password</label><input name=pass id=pass type=password placeholder='WiFi password'></div>"
         "<button type=submit>Save &amp; Connect</button></form>"
         "<p class=dim>The clock restarts and joins the network. 2.4 GHz only.</p>"
         "<script>"
         "const f=document.forms[0],pw=document.getElementById('pw'),on=document.getElementById('othname');"
         "f.onchange=e=>{const r=f.querySelector('input[name=ssid]:checked');if(!r)return;"
         "pw.style.display=r.dataset.open=='1'?'none':'';"
         "if(e.target==r)(r.id=='oth'?on:document.getElementById('pass')).focus()};"
         "on.onfocus=()=>{document.getElementById('oth').checked=true;f.onchange({})};"
         "</script>");
}

static void handleSave() {
  if (!webAuthed()) return;
  String ssid = web.arg("ssid");
  if (ssid.length() == 0) { ssid = web.arg("other"); ssid.trim(); }   // typed under "Other network…"
  if (ssid.length() == 0) { web.sendHeader("Location", "/wifi"); web.send(303); return; }
  prefs.putString("ssid", ssid);
  prefs.putString("pass", web.arg("pass"));
  web.send(200, "text/html", page("WFD WiFi Setup",
           "<h2>Saved — connecting…</h2><p>Rejoin your normal WiFi. The clock is restarting.</p>"));
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
// ---- microphone: sampled on core 0 while the app is off and something uses it ----
static void micTask(void *) {
  static uint16_t buf[mic::N];
  const uint32_t period = 1000000UL / mic::RATE;
  for (;;) {
    // Pause during an OTA update: this task busy-waits on core 0, which the WiFi stack needs then.
    bool want = millis() - appTime > 2000 && (micAuto || (gaAuto && grid::usesMic(gaAnim))) && !Update.isRunning();
    if (!want || micPause) {
      micBass = micTreble = micLevel = 0;
      for (int b = 0; b < mic::NBANDS; b++) micBands[b] = 0;
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    uint32_t next = micros();
    for (int i = 0; i < mic::N; i++) {          // 256 samples at 8 kHz = 32 ms
      while ((int32_t)(micros() - next) < 0) {}
      buf[i] = analogRead(MIC_PIN);
      next += period;
    }
    micDsp.process(buf);
    micBass = micDsp.colBass;
    micTreble = micDsp.colTreble;
    micLevel = (int)(micDsp.level * 100);
    for (int b = 0; b < mic::NBANDS; b++) micBands[b] = micDsp.bands[b];
    micFrameAt = millis();
    if ((int32_t)(micTraceUntil - micFrameAt) > 0)
      Serial.printf("T %lu %d %d %d %d %d %d\n", (unsigned long)micFrameAt, (int)micDsp.rms, (int)micDsp.lowAmp,
                    (int)micDsp.highAmp, micLevel, micBass, micTreble);
    vTaskDelay(1);                              // let core 0's idle task run
  }
}

// ---- microphone diagnostics ----
// Capture n samples at ~rate Hz with analogRead and print what the signal looks like.
static void micCapture(int n, int rate) {
  static uint16_t buf[2048];
  n = constrain(n, 16, 2048);
  uint32_t period = 1000000UL / rate, t0 = micros(), next = t0;
  for (int i = 0; i < n; i++) {
    while ((int32_t)(micros() - next) < 0) {}
    buf[i] = analogRead(MIC_PIN);
    next += period;
  }
  uint32_t us = micros() - t0;
  double mean = 0;
  int lo = 4095, hi = 0;
  for (int i = 0; i < n; i++) { mean += buf[i]; lo = min(lo, (int)buf[i]); hi = max(hi, (int)buf[i]); }
  mean /= n;
  double ac = 0;
  int zc = 0;
  for (int i = 0; i < n; i++) {
    double d = buf[i] - mean;
    ac += d * d;
    if (i && ((buf[i] >= mean) != (buf[i - 1] >= mean))) zc++;
  }
  int hist[9] = {0};          // 0, 1-3, 4-15, 16-63, 64-255, 256-1023, 1k-2k, 2k-3k, 3k+
  for (int i = 0; i < n; i++) {
    int v = buf[i];
    hist[v == 0 ? 0 : v < 4 ? 1 : v < 16 ? 2 : v < 64 ? 3 : v < 256 ? 4 : v < 1024 ? 5 : v < 2048 ? 6 : v < 3072 ? 7 : 8]++;
  }
  Serial.printf("mic n=%d rate=%dHz min=%d max=%d mean=%d acrms=%d zero-cross/s=%d\n", n,
                (int)(n * 1e6 / us), lo, hi, (int)mean, (int)sqrt(ac / n), (int)(zc * 1e6 / us));
  Serial.printf("  hist 0:%d 1-3:%d 4-15:%d 16-63:%d 64-255:%d 256-1023:%d 1k-2k:%d 2k-3k:%d 3k+:%d\n",
                hist[0], hist[1], hist[2], hist[3], hist[4], hist[5], hist[6], hist[7], hist[8]);
  Serial.print("  first 64:");
  for (int i = 0; i < 64; i++) Serial.printf(" %d", buf[i]);
  Serial.println();
}

// Every 100 ms: 400 samples (~10 kHz for 40 ms) -> min/max/mean and a peak-to-peak bar.
static void micLog(int secs) {
  for (int n = 0; n < secs * 10; n++) {
    uint32_t t0 = millis();
    int lo = 4095, hi = 0;
    long sum = 0;
    for (int i = 0; i < 400; i++) {
      int v = analogRead(MIC_PIN);
      lo = min(lo, v); hi = max(hi, v); sum += v;
      delayMicroseconds(80);
    }
    int pp = hi - lo, w = constrain(pp / 25, 0, 40);
    char bar[41];
    for (int i = 0; i < 40; i++) bar[i] = i < w ? '#' : '.';
    bar[40] = 0;
    Serial.printf("%3d.%ds min %4d max %4d mean %4ld pp %4d %s\n", n / 10, n % 10, lo, hi, sum / 400, pp, bar);
    while (millis() - t0 < 100) delay(1);
  }
}

// Choose what the grid plays when the app is off (turns that on), and persist it.
static void setGridAnim(int anim, int fps, int flag) {
  gaAnim = constrain(anim, 0, grid::N_ANIM - 1);
  gaFps = constrain(fps, 1, 30);
  gaFlag = flag ? 1 : 0;
  gaAuto = 1;
  gaIdx = 0;
  prefs.putInt("gaid", gaAnim); prefs.putInt("gfps", gaFps);
  prefs.putInt("gflag", gaFlag); prefs.putInt("gauto", gaAuto);
}

// ---- on-device message engine ----
static const char *DAY_ABBR[7] = {"MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"};
static const char *DAY_FULL[7] = {"MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY", "SATURDAY", "SUNDAY"};
static const char *MON_ABBR[12] = {"JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"};

static bool wxFresh() { return millis() - wxTime < 1200000; }

static String tempText() { return String(wxTemp) + (wxUnit ? "C" : "F"); }

// Expand {time} {time12} {date} {day} {temp} from the RTC / weather (same as the PC app).
static String expandMsg(String s) {
  if (s.indexOf('{') < 0) return s;
  const uint8_t *t = curT;
  int hh = bcd2dec(t[2] & 0x3F), mi = bcd2dec(t[1]);
  int dow = constrain((t[3] & 7) - 1, 0, 6), mo = constrain(bcd2dec(t[5] & 0x1F) - 1, 0, 11);
  char buf[16];
  snprintf(buf, sizeof buf, "%02d:%02d", hh, mi);
  s.replace("{time}", buf);
  snprintf(buf, sizeof buf, "%d:%02d%s", hh % 12 ? hh % 12 : 12, mi, hh < 12 ? "AM" : "PM");
  s.replace("{time12}", buf);
  snprintf(buf, sizeof buf, "%s %s %02d", DAY_ABBR[dow], MON_ABBR[mo], bcd2dec(t[4]));
  s.replace("{date}", buf);
  s.replace("{day}", DAY_FULL[dow]);
  s.replace("{temp}", wxFresh() ? tempText() : String("--"));
  return s;
}

static int rotSrc = N_SRC - 1, scrollPos = 0;
static uint32_t rotAt = 0, msgStepAt = 0;

static bool srcAvailable(int src) { return src != SRC_TEMP || wxFresh(); }

// One engine step: choose the source, render the next scroll position into engRam.
static void msgStep() {
  int src = msgMode;
  if (msgMode == MODE_ROTATE) {
    bool cur = (msgRot & (1 << rotSrc)) && srcAvailable(rotSrc);
    if (!cur || millis() - rotAt >= (uint32_t)msgDwell * 1000) {
      for (int k = 1; k <= N_SRC; k++) {               // next enabled + available source
        int c = (rotSrc + k) % N_SRC;
        if ((msgRot & (1 << c)) && srcAvailable(c)) {
          if (c != rotSrc || !cur) { rotSrc = c; scrollPos = 0; msgShown = ""; }
          break;
        }
      }
      rotAt = millis();
    }
    src = (msgRot & (1 << rotSrc)) && srcAvailable(rotSrc) ? rotSrc : SRC_TEMP;
  }
  if (src == SRC_TEMP) { engTemp = true; msgShown = wxFresh() ? tempText() : ""; return; }
  engTemp = false;
  String txt = expandMsg(src == SRC_DATE ? String("{date}") : msgText);
  txt.trim();
  if (txt.length() == 0) txt = " ";
  if (txt != msgShown) { msgShown = txt; scrollPos = 0; }
  if (txt.length() <= 6) {
    render6(txt, 0, engRam);
  } else {                                             // wrap with a 3-space gap, like the app
    String d = txt + "   ";
    render6(d + d, scrollPos % d.length(), engRam);
    scrollPos++;
  }
}

static void setMsgText(String t) {
  t.replace("\t", " ");
  t.trim();
  if (t.length() > MSG_TEXT_MAX) t = t.substring(0, MSG_TEXT_MAX);
  if (t == msgText) return;
  msgText = t;
  prefs.putString("omtext", msgText);
  msgStepAt = 0;                                       // show it now
}

static void setMsgCfg(int mode, int rot, int dwell, int scroll) {
  mode = constrain(mode, 0, MODE_ROTATE);
  rot &= (1 << N_SRC) - 1;
  dwell = constrain(dwell, 2, 600);
  scroll = constrain(scroll, 50, 5000);
  if (mode == msgMode && rot == msgRot && dwell == msgDwell && scroll == msgScroll) return;
  msgMode = mode; msgRot = rot; msgDwell = dwell; msgScroll = scroll;
  prefs.putInt("mmode", msgMode); prefs.putInt("mrot", msgRot);
  prefs.putInt("mdwell", msgDwell); prefs.putInt("mscroll", msgScroll);
  rotAt = 0; msgStepAt = 0;
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
  String s = "<h2>WFD Clock Control</h2><form method=POST action=/control>";
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
  s += "<div class=row><input type=checkbox name=micauto id=micauto " + String(micAuto ? "checked" : "") +
       "><label for=micauto>Spectrum from the clock's mic when the app is off</label></div>";
  s += "<h3>Grid <small class=dim>when the app is off</small></h3>";
  s += "<div class=row><input type=checkbox name=gauto id=gauto " + String(gaAuto ? "checked" : "") +
       "><label for=gauto>Play an animation</label></div>";
  s += "<label>Animation</label><select name=ganim>";
  for (int i = 1; i < grid::N_ANIM; i++) {
    String n = grid::NAMES[i];
    n.setCharAt(0, n[0] - 32);                       // "spin" -> "Spin"
    s += "<option value=" + String(i) + (gaAnim == i ? " selected" : "") + ">" + n + "</option>";
  }
  s += "<option value=0" + String(gaAnim == grid::CUSTOM ? " selected" : "") + (gaN ? "" : " disabled") +
       ">Drawn in the app (" + String(gaN) + " frames)</option></select>";
  s += "<label>Speed <small>(frames per second)</small></label>"
       "<input type=range name=gfps min=2 max=25 value=" + String(gaFps) + ">";
  s += "<div class=row><input type=checkbox name=gflag id=gflag " + String(gaFlag ? "checked" : "") +
       "><label for=gflag>Light the border frame</label></div>";

  s += "<h3>Message line <small class=dim>when the app is off</small></h3>";
  s += "<div class=row><input type=checkbox name=omauto id=omauto " + String(omAuto ? "checked" : "") +
       "><label for=omauto>Use these settings (off = temperature only)</label></div>";
  const char *modes[] = {"Temperature", "Text", "Date", "Rotate"};
  s += "<label>Show</label><select name=mmode>";
  for (int i = 0; i <= MODE_ROTATE; i++)
    s += "<option value=" + String(i) + (msgMode == i ? " selected" : "") + ">" + modes[i] + "</option>";
  s += "</select>";
  s += "<label>Text</label><input name=omtext maxlength=" + String(MSG_TEXT_MAX) + " value='" + esc(msgText) + "'>";
  s += "<p class=dim style='margin:4px 0;font-size:13px'>Placeholders: {time} {time12} {date} {day} {temp}</p>";
  s += "<label>Rotate through</label><div class=row style=margin-top:4px>";
  const char *srcs[] = {"Temperature", "Text", "Date"};
  for (int i = 0; i < N_SRC; i++)
    s += "<input type=checkbox name=rot" + String(i) + " id=rot" + String(i) + ((msgRot >> i) & 1 ? " checked" : "") +
         "><label for=rot" + String(i) + ">" + srcs[i] + "</label>";
  s += "</div>";
  s += "<label>Seconds per item (rotate)</label><input type=number name=mdwell min=2 max=600 value=" + String(msgDwell) + ">";
  s += "<label>Scroll speed <small>(ms per step, lower = faster)</small></label>"
       "<input type=range name=mscroll min=100 max=1000 step=50 value=" + String(msgScroll) + ">";
  s += "<p class=dim style=font-size:13px>Now showing: <b>" + esc(msgShown.length() ? msgShown : String("—")) + "</b></p>";
  s += "<button type=submit>Apply</button></form>";
  s += "<p class=dim><a href=/>Dashboard</a> &middot; <a href=/status>Status</a> &middot; <a href=/update>Firmware update</a></p>";
  return page("WFD Control", s);
}
// ============================= settings (JSON API + phone form) =============================
// One table of settings, shared by POST /api/set and the /control form. Names match the PC app
// where the meaning is the same; clock-only settings for "when the app is off" are standalone_*.
// applySettings() checks every key first and changes nothing unless all are valid.
static const char *MSG_MODE_NAMES[] = {"weather", "text", "date", "rotate"};   // index = SRC_* / MODE_ROTATE

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
    if ((i = indexOf(MSG_MODE_NAMES, 4, v.as<const char *>())) < 0) { err = "expected weather, text, date or rotate"; return false; }
    pd.mode = i; pd.msg = true;
  } else if (!strcmp(k, "standalone_rotate")) {
    if (!v.is<JsonArrayConst>()) { err = "expected a list of weather/text/date"; return false; }
    int rot = 0;
    for (JsonVariantConst s : v.as<JsonArrayConst>()) {
      if ((i = indexOf(MSG_MODE_NAMES, N_SRC, s.as<const char *>())) < 0) { err = "expected a list of weather/text/date"; return false; }
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
  } else {
    err = "unknown setting";
    return false;
  }
  return true;
}

// Validate everything in `in`; only if all keys are valid, apply them. Errors/warnings go to `out`.
static bool applySettings(JsonObjectConst in, JsonDocument &out) {
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

static void configJson(JsonObject c);

// Same settings and checks as /api/set, for the PC app over USB. Replies are one line each.
static void serialCfg(const String &json) {
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
  cfg["_protocol"] = WFD_PROTOCOL;
  cfg["_ip"] = WiFi.status() == WL_CONNECTED ? WiFi.localIP().toString() : String();   // "" off WiFi
  Serial.print(json.length() ? "CFGOK " : "CFG ");
  serializeJson(cfg, Serial);
  Serial.println();
}

static void configJson(JsonObject c) {
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
}

static void stateJson(JsonObject s) {
  s["fw"] = FW_BUILD;
  s["protocol"] = WFD_PROTOCOL;
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
  s["app_connected"] = millis() - appTime < 2000;
  s["message"] = msgShown;
  s["stored_frames"] = gaN;
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

// Store drawn grid frames (rows: n x 7 bytes) and play them when the app is off.
static void storeFrames(const uint8_t *rows, int n, int fps, int flag) {
  gaN = constrain(n, 0, GA_MAX);
  gaIdx = 0;
  for (int i = 0; i < gaN * 7; i++) gaFrames[i] = rows[i] & 0x7F;
  prefs.putBytes("ganim", gaFrames, gaN * 7);
  prefs.putInt("gn", gaN);
  setGridAnim(grid::CUSTOM, fps, flag);
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
  storeFrames(rows, n, in["fps"] | gaFps, (in["frame"] | false) ? 1 : 0);
  out["ok"] = true;
  out["frames"] = gaN;
  sendJson(200, out);
}

// Live display commands from the app over WiFi. One packet per frame, e.g.
//   "WFD1 <key>\nSPEC 3 5\nICON 0\nGRID 0 0 0 8 0 0 0 1"
static void netPoll() {
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
  if (zip.length() && zip != prefs.getString("zip", "")) in["location"] = zip;
  if (web.hasArg("omtext")) in["offline_text"] = web.arg("omtext");
  if (web.hasArg("mmode")) {
    in["standalone_msg_mode"] = MSG_MODE_NAMES[constrain(web.arg("mmode").toInt(), 0, MODE_ROTATE)];
    JsonArray rot = in["standalone_rotate"].to<JsonArray>();
    for (int i = 0; i < N_SRC; i++) if (web.hasArg("rot" + String(i))) rot.add(MSG_MODE_NAMES[i]);
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
    String msg;
    for (JsonPairConst kv : out["error"].as<JsonObjectConst>())
      msg += "<li>" + esc(kv.key().c_str()) + ": " + esc(kv.value().as<const char *>()) + "</li>";
    web.send(400, "text/html", page("WFD Control", "<h2 class=warn>Not saved</h2><ul>" + msg +
                                    "</ul><p><a href=/control>Back</a></p>"));
    return;
  }
  web.sendHeader("Location", "/control");
  web.send(303);
}

static bool otaOk = false;             // set by the upload handler, read by the final POST handler
static void handleUpdateUpload() {
  HTTPUpload &up = web.upload();
  if (up.status == UPLOAD_FILE_START) {
    otaOk = false;
    if (webPass.length() && !web.authenticate("admin", webPass.c_str())) return;   // POST handler sends the 401
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
    "<h2>Firmware Update</h2><form method=POST action=/update enctype=multipart/form-data>"
    "<input type=file name=fw accept=.bin><button type=submit>Upload &amp; Flash</button></form>"
    "<p class=dim>Upload a firmware .bin for this clock. It reboots when done. <a href=/>Back</a></p>";

static void startWebRoutes() {
  web.on("/", []() {                             // setup portal in AP mode, else the dashboard
    if (apMode) return web.send(200, "text/html", setupPage());
    if (!webAuthed()) return;                      // so the browser has the login for the API calls
    web.sendHeader("Content-Encoding", "gzip");
    web.send_P(200, "text/html", (const char *)DASHBOARD_GZ, DASHBOARD_GZ_LEN);
  });
  web.on("/status", []() { web.send(200, "text/html", statusPage()); });
  web.on("/draw", HTTP_GET, []() {               // grid animator (the same page the PC app serves)
    if (!webAuthed()) return;
    web.sendHeader("Content-Encoding", "gzip");
    web.send_P(200, "text/html", (const char *)DRAW_GZ, DRAW_GZ_LEN);
  });
  web.on("/save", HTTP_POST, handleSave);
  web.on("/wifi", HTTP_GET, []() {                // change networks (also reachable in AP mode)
    if (!webAuthed()) return;
    web.send(200, "text/html", setupPage());
  });
  web.on("/rescan", []() {
    WiFi.scanDelete(); WiFi.scanNetworks(true);
    web.sendHeader("Location", "/wifi"); web.send(303);
  });
  web.on("/control", HTTP_GET, []() { if (webAuthed()) web.send(200, "text/html", controlPage()); });
  web.on("/control", HTTP_POST, handleControlPost);
  web.on("/api/status", HTTP_GET, handleApiStatus);
  web.on("/api/set", HTTP_POST, handleApiSet);
  web.on("/api/frames", HTTP_POST, handleApiFrames);
  web.on("/api/frames", HTTP_GET, []() {         // the stored drawing, for the animator to start from
    JsonDocument out;
    JsonArray fr = out["frames"].to<JsonArray>();
    for (int i = 0; i < gaN; i++) {
      JsonArray row = fr.add<JsonArray>();
      for (int r = 0; r < 7; r++) row.add(gaFrames[i * 7 + r]);
    }
    out["fps"] = gaFps;
    out["frame"] = (bool)gaFlag;
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
    for (JsonVariantConst v : in["rows"].as<JsonArrayConst>()) if (r < 7) gridRows[r++] = v.as<int>() & 0x7F;
    while (r < 7) gridRows[r++] = 0;
    gridFrame = in["frame"].as<int>() ? 1 : 0;
    gridTime = millis();                           // shows like a live GRID for 1.5 s
    dirty = true;
    out["ok"] = true;
    sendJson(200, out);
  });
  const char *hdrs[] = {"X-WFD"};
  web.collectHeaders(hdrs, 1);
  web.on("/update", HTTP_GET, []() { if (webAuthed()) web.send(200, "text/html", page("WFD Update", UPDATE_FORM)); });
  web.on("/update", HTTP_POST, []() {
    if (!webAuthed()) return;
    bool ok = otaOk && !Update.hasError();
    if (!ok) Update.printError(Serial);
    web.send(ok ? 200 : 500, "text/html", ok
      ? page("WFD Update", "<h2>Update OK — rebooting…</h2>", "<meta http-equiv=refresh content='6;url=/'>")
      : page("WFD Update", "<h2 class=warn>Update FAILED</h2><p><a href=/update>Try again</a></p>"));
    if (!ok) return;                    // keep running the current firmware
    web.client().flush();               // finish the reply and close cleanly, so the uploader
    web.client().stop();                //   doesn't see "connection reset" from the reboot
    delay(700);
    ESP.restart();
  }, handleUpdateUpload);
  web.onNotFound([]() {                          // captive portal: every URL is the setup page
    if (apMode) web.send(200, "text/html", setupPage());
    else web.send(404, "text/plain", "not found");
  });
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
  String s = "<h2>WFD Clock <small class=dim>fw: OTA-OK</small></h2>";
  s += "<p>Time: <b>" + String(tm) + "</b></p>";
  s += "<p>NTP: <b>" + String(ntpSynced ? "synced" : "waiting…") + "</b> &nbsp; WiFi: <b>" + esc(WiFi.SSID()) +
       "</b> (" + WiFi.localIP().toString() + ", " + String(WiFi.RSSI()) + " dBm)</p>";
  const char *cond = wxCode == 1 ? "Sunny" : wxCode == 2 ? "Cloudy" : wxCode == 3 ? "Rain" : "—";
  bool wxfresh = millis() - wxTime < 1200000;
  s += "<p>Weather: <b>" + (wxfresh ? String(wxTemp) + (wxUnit ? "C " : "F ") + cond : String("—")) +
       "</b> &nbsp; loc " + (g_lat.length() ? g_lat + "," + g_lon : String("unset")) + "</p>";
  s += "<p><a href=/ style=font-weight:600>⚙ Dashboard</a> &nbsp; <a href=/control>Basic controls</a></p>";
  s += "<p><a href=/update>Firmware update</a> &nbsp; <a href=/wifi class=warn>Change WiFi</a></p>";
  return page("WFD Clock", s);
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
  // stored frames keep playing after the upgrade; otherwise default to a built-in animation
  gaAnim = constrain(prefs.getInt("gaid", gaN > 0 ? grid::CUSTOM : grid::SPIN), 0, grid::N_ANIM - 1);
  webPass = prefs.getString("webpass", "");
  micAuto = prefs.getInt("micauto", 1);
  analogSetPinAttenuation(MIC_PIN, ADC_11db);   // dead zone hides the room's hum (see mic_dsp.h)
  gEng.bands = micBands;
  xTaskCreatePinnedToCore(micTask, "mic", 4096, nullptr, 1, nullptr, 0);
  omAuto = prefs.getInt("omauto", 0);    // restore the message-line engine
  msgMode = constrain(prefs.getInt("mmode", SRC_TEXT), 0, MODE_ROTATE);
  msgRot = prefs.getInt("mrot", 0b111);
  msgDwell = prefs.getInt("mdwell", 8);
  msgScroll = prefs.getInt("mscroll", 350);
  msgText = prefs.getString("omtext", "WFD CLOCK");
  prefs.remove("om"); prefs.remove("omn"); prefs.remove("omfps");   // pre-rendered frames: no longer used
  wifiBegin();
  Serial.printf("wfd-clock ready, style %d, bright %d, grid %d auto %d, msg mode %d auto %d, wifi %s\n",
                specStyle, brightness, gaN, gaAuto, msgMode, omAuto, apMode ? "setup-AP" : "connecting");
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
    else if (line.length() < 1024) line += c;   // room for CFGSET {json}
  }
  wifiLoop();
  if (netUp) netPoll();
  checkButtons();
  // advance stored animations only when the app is off
  bool redraw = false;
  bool appOff = millis() - appTime > 2000;
  if (appOff && gaAuto && gridReady() &&
      millis() - gaStep > (uint32_t)(1000 / (gaFps < 1 ? 1 : gaFps))) {
    gaStep = millis();
    if (gaAnim == grid::CUSTOM) gaIdx = (gaIdx + 1) % gaN;
    else gEng.frame(gaAnim, gEngRows);
    redraw = true;
  }
  if (appOff && omAuto && (msgStepAt == 0 || millis() - msgStepAt >= (uint32_t)msgScroll)) {
    msgStepAt = millis();
    msgStep();
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
