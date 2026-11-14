// State shared between the firmware's source files, grouped by what owns it: one struct per
// group, one instance each, defined in clock_state.cpp (FW_BUILD and fwProtocol in main.cpp).
// Groups only a few files use live in their owner's header instead: weather and forecast
// (weather_fetch.h), alarms/timer/alert (buttons_alerts.h), the mic task's results
// (microphone.h), the upload log (ota_update.h) and the web server (web_routes.h). Which of
// these values are saved in flash, under which key, is in nvs_store.cpp. State only one file
// uses stays `static` in that file.
#pragma once
#include <Arduino.h>
#include "grid_engine.h"

// When something last happened, starting out as "never": a millis() of 0 isn't "long ago" in
// the first seconds after boot. within() compares with unsigned subtraction (wrap-safe).
struct Stamp {
  uint32_t at = 0;
  bool set = false;
  void mark() { at = millis(); set = true; }
  void clear() { set = false; }
  bool within(uint32_t ms) const { return set && millis() - at < ms; }
};

extern const char *FW_BUILD;          // "wfd-clock <date> <time>" (main.cpp)
extern const int fwProtocol;          // WFD_PROTOCOL (main.cpp)

#define N_STYLES 5

// ---- the panel: what draw() reads besides the groups below, and its saved settings ----
struct Display {
  uint8_t now[7] = {0};               // last RTC reading
  bool dirty = false;                 // a command changed the display; loop() redraws once
  int style = 4;                      // spectrum: 1=bars 2=top-down 3=center-out 4=peak dot 5=bars+peak
  int clock24 = 1;                    // 1 = 24-hour, 0 = 12-hour display
  int brightness = 6;                 // 1..16 (TM1680 PWM)
  // A short notice on the message line (the IP or the setup AP after holding a button); scrolls
  // when longer than 6 characters and goes away by itself.
  String infoText;
  uint32_t infoUntil = 0, infoStepAt = 0;
  int infoPos = 0;
};
extern Display disp;
inline bool infoOn() { return disp.infoText.length() && (int32_t)(disp.infoUntil - millis()) > 0; }

// ---- the live frame from the app (serial or UDP display commands) ----
static const uint32_t APP_ON_MS = 2000;   // the app counts as on this long after a command
struct LiveFrame {
  int specA = 0, specB = 0;           // spectrum column levels 0..12
  Stamp specTime;                     // last SPEC frame
  int barCpu = 0, barGpu = 0, barRam = 0;   // 0..100 percent
  Stamp barsTime;
  uint8_t msgRam[12] = {0};           // raw glyphs for the 6 message chars (lo,hi x6 = bytes 0..11)
  Stamp msgTime;
  uint8_t gridRows[7] = {0};          // bit c (0..6) of row r = cell (r,c)
  uint8_t gridFrame = 0;
  Stamp gridTime;
  uint32_t iconMask = 0;              // top-row icons bitmask
  Stamp iconTime;
  int logoOn = 0;
  Stamp logoTime;
  Stamp appTime;                      // last time any serial command arrived (app alive)
};
extern LiveFrame live;

// ---- when the app is off: the message line engine, the grid animation, the mic columns ----
enum { SRC_TEMP = 0, SRC_TEXT = 1, SRC_DATE = 2, MODE_ROTATE = 3, SRC_FORECAST = 4, N_SRC = 5 };   // 3 = rotate (a mode, not a source; kept for saved settings)
#define MSG_TEXT_MAX 96
#define GA_MAX 80
struct Standalone {
  // message line (settings)
  int omAuto = 0;                     // 1 = run this engine when the app is off, 0 = just the temperature
  int msgMode = SRC_TEXT;             // SRC_TEMP / SRC_TEXT / SRC_DATE / MODE_ROTATE / SRC_FORECAST
  int msgRot = 0b111;                 // rotate: bit per SRC_*
  int msgDwell = 8;                   // rotate: seconds per source
  int msgScroll = 350;                // ms per scroll step
  String msgText = "WFD CLOCK";       // text source; {time} {time12} {date} {day} {temp} expand live
  // message line (engine output)
  uint8_t engRam[12] = {0};           // engine output glyphs
  bool engTemp = true;                // engine says: show the temperature (drawTemp) instead of engRam
  String msgShown;                    // text currently on the message line (MSGINFO, phone page)
  // grid: the stored drawing (plays from flash) or a built-in animation
  uint8_t gaFrames[GA_MAX * 7] = {0};
  int gaN = 0, gaFps = 8, gaFlag = 0, gaAuto = 0;
  int gaIdx = 0;
  int gaAnim = grid::SPIN;            // what plays when the app is off: grid::CUSTOM = gaFrames
  uint8_t gEngRows[7] = {0};          // current built-in animation frame (audio/mouth read the mic bands)
  // spectrum columns
  int micAuto = 1;                    // spectrum columns from the mic when the app is off
  int micSpin = 0;                    // how the mic is sampled: 0 = timer (sleeps between samples), 1 = spin (busy-wait)
};
extern Standalone standalone;
inline bool gridReady() { return standalone.gaAnim != grid::CUSTOM || standalone.gaN > 0; }

// ---- network: WiFi / NTP, and the PC app over WiFi ----
// UDP packets "WFD1 <key>\n<command>\n<command>..." (key = web password, if set). Only
// live-display commands are accepted; settings go through the HTTP API.
static const uint16_t UDP_PORT = 4210;
static const char *const HOSTNAME = "wfd-clock";   // -> wfd-clock.local (mDNS)
struct Net {
  bool apMode = false;
  bool ntpSynced = false;
  String tz;                          // POSIX TZ (saved)
  bool up = false;                    // the UDP socket is open
  uint32_t udpPackets = 0, udpRejected = 0;
  String webPass;                     // "webpass" (saved; checked 30x a second)
};
extern Net net;
