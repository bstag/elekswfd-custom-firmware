// The serial command set: handle() runs one command line (USB, or a display command from UDP).
#include <Arduino.h>
#include <WiFi.h>
#include <time.h>
#include "clock_state.h"
#include "clock_display.h"
#include "clock_settings.h"
#include "microphone.h"
#include "nvs_store.h"
#include "standalone.h"
#include "weather_fetch.h"
#include "serial_cmds.h"

// GCLR / GADD / GCOMMIT: a drawing being uploaded over USB
static uint8_t upBuf[GA_MAX * 7];
static int upN = 0;

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

void handle(const String &s) {
  live.appTime.mark();                     // any command means the app is alive
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
    live.specA = constrain(v[0], 0, 12);
    live.specB = constrain(v[1], 0, 12);
    live.specTime.mark();
    disp.dirty = true;
  } else if (cmd == "BARS") {
    int v[3] = {0, 0, 0};
    parseInts(arg, v, 3);
    live.barCpu = constrain(v[0], 0, 100);
    live.barGpu = constrain(v[1], 0, 100);
    live.barRam = constrain(v[2], 0, 100);
    live.barsTime.mark();
    disp.dirty = true;
  } else if (cmd == "MSG") {
    int v[12] = {0};
    parseInts(arg, v, 12);
    for (int k = 0; k < 12; k++) live.msgRam[k] = (uint8_t)v[k];
    live.msgTime.mark();
    disp.dirty = true;
  } else if (cmd == "GRID") {
    int v[8] = {0};
    parseInts(arg, v, 8);
    for (int k = 0; k < 7; k++) live.gridRows[k] = (uint8_t)(v[k] & 0x7F);
    live.gridFrame = v[7] ? 1 : 0;
    live.gridTime.mark();
    disp.dirty = true;
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
    Serial.printf("OK stored %d frames\n", standalone.gaN);
  } else if (cmd == "GANIM") {                  // "GANIM <name> [fps] [frame]"  built-in animation
    int sp2 = arg.indexOf(' ');
    String name = sp2 < 0 ? arg : arg.substring(0, sp2);
    int v[2] = {standalone.gaFps, standalone.gaFlag};
    if (sp2 > 0) parseInts(arg.substring(sp2 + 1), v, 2);
    int a = grid::animFromName(name.c_str());
    if (a < 0) Serial.printf("ERR unknown animation '%s'\n", name.c_str());
    else { setGridAnim(a, v[0], v[1]); Serial.printf("OK ganim %s %d %d\n", grid::NAMES[standalone.gaAnim], standalone.gaFps, standalone.gaFlag); }
  } else if (cmd == "GINFO") {
    const uint8_t *r = standalone.gaAnim == grid::CUSTOM ? &standalone.gaFrames[standalone.gaIdx * 7] : standalone.gEngRows;
    Serial.printf("grid auto=%d anim=%s fps=%d frame=%d stored=%d rows=%d %d %d %d %d %d %d\n", standalone.gaAuto,
                  grid::NAMES[standalone.gaAnim], standalone.gaFps, standalone.gaFlag, standalone.gaN, r[0], r[1], r[2], r[3], r[4], r[5], r[6]);
  } else if (cmd == "GAUTO") {
    standalone.gaAuto = arg.toInt() ? 1 : 0;
    prefSave(P_GAUTO);
    Serial.printf("OK auto %d\n", standalone.gaAuto);
  } else if (cmd == "OMTEXT") {                 // "OMTEXT <text>"  message text, placeholders allowed
    setMsgText(arg);
    Serial.printf("OK omtext '%s'\n", standalone.msgText.c_str());
  } else if (cmd == "MSGCFG") {                 // "MSGCFG <mode> <rotmask> <dwell s> <scroll ms>"
    int v[4] = {standalone.msgMode, standalone.msgRot, standalone.msgDwell, standalone.msgScroll};
    parseInts(arg, v, 4);
    setMsgCfg(v[0], v[1], v[2], v[3]);
    Serial.printf("OK msgcfg %d %d %d %d\n", standalone.msgMode, standalone.msgRot, standalone.msgDwell, standalone.msgScroll);
  } else if (cmd == "MSGINFO") {                // what the engine is showing right now (for testing)
    Serial.printf("msg auto=%d mode=%d rot=%d dwell=%d scroll=%d src=%s text='%s'\n", standalone.omAuto, standalone.msgMode,
                  standalone.msgRot, standalone.msgDwell, standalone.msgScroll, standalone.engTemp ? "temp" : "glyphs", standalone.msgShown.c_str());
  } else if (cmd == "MIC") {                    // mic diagnostics: one fast capture + stats
    micState.pause = true; delay(40);                 // let micTask finish its frame
    if (arg.length()) {                         // "MIC <0..3>": ADC attenuation 0/2.5/6/11 dB
      const adc_attenuation_t at[] = {ADC_0db, ADC_2_5db, ADC_6db, ADC_11db};
      analogSetPinAttenuation(MIC_PIN, at[constrain(arg.toInt(), 0, 3)]);
    }
    micCapture(2048, 10000);
    analogSetPinAttenuation(MIC_PIN, ADC_11db);
    micState.pause = false;
  } else if (cmd == "MICTRACE") {               // "MICTRACE <s>": per frame "T ms rms low high level% bass treble"
    micState.traceUntil = millis() + 1000UL * constrain(arg.toInt(), 1, 120);
    live.appTime.clear();                            // this command shouldn't count as the app being on
  } else if (cmd == "MICLOG") {                 // "MICLOG <s>": level bar every 100 ms
    micState.pause = true; delay(40);
    micLog(constrain(arg.toInt(), 1, 60));
    micState.pause = false;
  } else if (cmd == "OMAUTO") {
    standalone.omAuto = arg.toInt() ? 1 : 0;
    prefSave(P_OMAUTO);
    Serial.printf("OK ommsg auto %d\n", standalone.omAuto);
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
                  net.apMode ? "AP" : (WiFi.status() == WL_CONNECTED ? "connected" : "connecting"),
                  (int)WiFi.status(), prefString(P_SSID).c_str(),
                  WiFi.localIP().toString().c_str(), WiFi.RSSI(), net.ntpSynced);
  } else if (cmd == "WIFISET") {                // "WIFISET <ssid>\t<pass>" (tab-separated)
    int tab = arg.indexOf('\t');
    if (tab > 0) {
      prefPutString(P_SSID, arg.substring(0, tab));
      prefPutString(P_PASS, arg.substring(tab + 1));
      Serial.println("OK wifi saved, restarting");
      delay(300); ESP.restart();
    } else Serial.println("ERR need ssid<TAB>pass");
  } else if (cmd == "GEO") {                    // "GEO <lat> <lon> <unit>"  set weather location
    int a = arg.indexOf(' '), b = arg.indexOf(' ', a + 1);
    if (a > 0 && b > 0) {
      wx.lat = arg.substring(0, a);
      wx.lon = arg.substring(a + 1, b);
      wx.units = arg.substring(b + 1).toInt() ? 1 : 0;
      prefSave(P_LAT); prefSave(P_LON); prefSave(P_WUNIT);
      wx.lastFetch = 0;                          // fetch now
      Serial.println("OK geo");
    } else Serial.println("ERR geo");
  } else if (cmd == "TZ") {
    net.tz = arg;
    prefSave(P_TZ);
    configTzTime(net.tz.c_str(), "pool.ntp.org", "time.nist.gov");
    Serial.println("OK tz");
  } else if (cmd == "WIFISSID") {               // fix just the network name, keep the password
    prefPutString(P_SSID, arg);
    Serial.println("OK ssid updated, restarting");
    delay(300); ESP.restart();
  } else if (cmd == "BYE") {                    // the app is quitting: take over now, not in 2 s
    live.appTime.clear();
    live.specTime.clear(); live.barsTime.clear(); live.msgTime.clear(); live.gridTime.clear(); live.iconTime.clear(); live.logoTime.clear();
    Serial.println("OK bye");
  } else if (cmd == "WEBPASS") {                // web password for the pages + WiFi stream ("" = open)
    // USB only on purpose: not a setting (/api/set), not in the UDP whitelist. Physical access
    // is what lets you change it. Replies "PWOK {...}" / "PWERR {...}" for the PC app.
    bool ok = arg.length() <= 64;
    for (char c : arg) ok = ok && c > 32 && c < 127;   // printable, no spaces (UDP/basic auth trim)
    if (!ok) {
      Serial.println("PWERR {\"error\":\"up to 64 printable characters, no spaces\"}");
    } else {
      net.webPass = arg;
      prefSave(P_WEBPASS);
      Serial.printf("PWOK {\"set\":%s}\n", arg.length() ? "true" : "false");   // user: admin
    }
  } else if (cmd == "WIFIRESET") {
    prefRemove(P_SSID); prefRemove(P_PASS);
    Serial.println("OK cleared, restarting");
    delay(300); ESP.restart();
  } else if (cmd == "ICON") {
    live.iconMask = (uint32_t)arg.toInt();
    live.iconTime.mark();
    disp.dirty = true;
  } else if (cmd == "LOGO") {
    live.logoOn = arg.toInt() ? 1 : 0;
    live.logoTime.mark();
    disp.dirty = true;
  } else if (cmd == "CLK24") {
    disp.clock24 = arg.toInt() ? 1 : 0;
    prefSave(P_CLK24);
    disp.dirty = true;
  } else if (cmd == "BRIGHT") {
    setBrightness(arg.toInt());
    Serial.printf("OK bright %d\n", disp.brightness);
  } else if (cmd == "WX") {
    int v[3] = {0, 0, 0};
    parseInts(arg, v, 3);
    wx.code = constrain(v[0], 0, 3);
    wx.temp = constrain(v[1], -99, 199);
    wx.unit = v[2] ? 1 : 0;
    wx.time.mark();
    disp.dirty = true;
  } else if (cmd == "STYLE") {
    int v = arg.toInt();
    if (v >= 1 && v <= N_STYLES) { setStyle(v); disp.dirty = true; Serial.printf("OK style %d\n", disp.style); }
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
