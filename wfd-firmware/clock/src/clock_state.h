// State shared between the firmware's source files: declared here, defined in clock_state.cpp
// (FW_BUILD and fwProtocol in main.cpp). State only one file uses stays `static` in that file.
#pragma once
#include <Arduino.h>
#include <Preferences.h>
#include <WebServer.h>
#include "extras.h"
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

extern Preferences prefs;
extern const char *FW_BUILD;          // "wfd-clock <date> <time>" (main.cpp)
extern const int fwProtocol;          // WFD_PROTOCOL (main.cpp)

// WiFi / NTP state
extern WebServer web;
extern bool apMode;
extern bool ntpSynced;
extern String g_tz;
extern String g_lat, g_lon;           // weather location (empty = none)
extern int g_wunit;                   // 0=F 1=C
extern uint32_t lastWxFetch;

extern uint8_t curT[7];               // last RTC reading
extern int specA, specB;              // spectrum column levels 0..12
extern Stamp specTime;                // last SPEC frame
extern int barCpu, barGpu, barRam;    // 0..100 percent
extern Stamp barsTime;
extern int wxCode, wxTemp, wxUnit;    // code 0=none 1=sunny 2=cloud 3=rains; unit 0=F 1=C
extern Stamp wxTime;                  // last weather (WX from the PC or our own fetch)
inline bool wxFresh() { return wxTime.within(1200000); }   // PC every 60 s, own fetch every 15 min
extern int wxScene;                   // grid::Scene from our own fetch (WMO code + day/night), -1 = none yet
extern uint32_t wxSceneTime;
extern int wxScenePick;               // "weather_scene" setting: a grid::Scene, or -1 = auto
// forecast from our own fetch: day 0 = today (Open-Meteo daily, timezone=auto)
extern int fcN, fcHi[4], fcLo[4], fcCode[4];
extern uint32_t fcTime;
inline bool fcFresh() { return fcN >= 2 && millis() - fcTime < 10800000; }
// alarms (NVS "alarms"), the countdown timer and the alert that both raise
extern extras::Alarm alarms[extras::N_ALARMS];
extern uint32_t cdEnd;                // millis() when the timer ends, 0 = no timer
enum { ALERT_NONE = 0, ALERT_ALARM, ALERT_TIMER };
extern int alertKind;
inline uint32_t timerLeft() { return cdEnd && (int32_t)(cdEnd - millis()) > 0 ? (cdEnd - millis() + 999) / 1000 : 0; }

// A short notice on the message line (the IP or the setup AP after holding a button); scrolls
// when longer than 6 characters and goes away by itself.
extern String infoText;
extern uint32_t infoUntil, infoStepAt;
extern int infoPos;
inline bool infoOn() { return infoText.length() && (int32_t)(infoUntil - millis()) > 0; }

extern uint8_t msgRam[12];            // raw glyphs for the 6 message chars (lo,hi x6 = bytes 0..11)
extern Stamp msgTime;

// on-device message line (runs when the app isn't driving the message chars)
enum { SRC_TEMP = 0, SRC_TEXT = 1, SRC_DATE = 2, MODE_ROTATE = 3, SRC_FORECAST = 4, N_SRC = 5 };   // 3 = rotate (a mode, not a source; kept for saved settings)
#define MSG_TEXT_MAX 96
extern int omAuto;                    // 1 = run this engine when the app is off, 0 = just the temperature
extern int msgMode;                   // SRC_TEMP / SRC_TEXT / SRC_DATE / MODE_ROTATE
extern int msgRot;                    // rotate: bit per SRC_*
extern int msgDwell;                  // rotate: seconds per source
extern int msgScroll;                 // ms per scroll step
extern String msgText;                // text source; {time} {time12} {date} {day} {temp} expand live
extern uint8_t engRam[12];            // engine output glyphs
extern bool engTemp;                  // engine says: show the temperature (drawTemp) instead of engRam
extern String msgShown;               // text currently on the message line (MSGINFO, phone page)
extern uint8_t gridRows[7];           // bit c (0..6) of row r = cell (r,c)
extern uint8_t gridFrame;
extern Stamp gridTime;
extern Stamp appTime;                 // last time any serial command arrived (app alive)
static const uint32_t APP_ON_MS = 2000;   // the app counts as on this long after a command
extern uint32_t iconMask;             // top-row icons bitmask
extern Stamp iconTime;
extern int logoOn;
extern Stamp logoTime;
extern bool dirty;                    // a command changed the display; loop() redraws once

extern int specStyle;                 // 1=bars 2=top-down 3=center-out 4=peak dot 5=bars+peak
#define N_STYLES 5
extern int clock24;                   // 1 = 24-hour, 0 = 12-hour display
extern int brightness;                // 1..16 (TM1680 PWM)

// stored standalone animation (plays from flash when the app isn't driving the grid)
#define GA_MAX 80
extern uint8_t gaFrames[GA_MAX * 7];
extern int gaN, gaFps, gaFlag, gaAuto;
extern int gaIdx;
extern int gaAnim;                    // what plays when the app is off: grid::CUSTOM = gaFrames
extern uint8_t gEngRows[7];           // current built-in animation frame (audio/mouth read micBands)
inline bool gridReady() { return gaAnim != grid::CUSTOM || gaN > 0; }

static const int MIC_PIN = 32;        // microphone: GPIO32 / ADC1_CH4, behind an op-amp
extern int micAuto;                   // spectrum columns from the mic when the app is off
extern volatile int micBass, micTreble, micLevel;   // latest results
extern double micBands[];             // [mic::NBANDS] grid audio/mouth input
extern volatile uint32_t micFrameAt;  // millis of the last frame
extern volatile bool micPause;        // MIC/MICLOG use the ADC
extern volatile uint32_t micTraceUntil;   // MICTRACE: print every frame

// The PC app over WiFi: UDP packets "WFD1 <key>\n<command>\n<command>..." (key = web password,
// if set). Only live-display commands are accepted; settings go through the HTTP API.
static const uint16_t UDP_PORT = 4210;
static const char *const HOSTNAME = "wfd-clock";   // -> wfd-clock.local (mDNS)
extern bool netUp;
extern uint32_t udpPackets, udpRejected;
extern String webPass;                // cached "webpass" (checked 30x a second)

// The last OTA upload, for diagnosing stalls: saved to NVS ("otalog") when an upload ends, so it
// survives the restart after a good update and is still there after an aborted one. Shown as
// state.ota in /api/status and printed as an "OTA ..." line on serial.
struct OtaLog {
  char result[56];                // ok | no auth | begin failed: .. | write failed: .. | end failed: .. | aborted
  uint32_t bytes, chunks, ms;     // received, pieces, start -> end
  uint32_t maxGapMs, gapAt;       // longest pause between pieces, and how far in (bytes) it came
  int32_t rssiStart, rssiEnd;     // WiFi signal (dBm)
  uint32_t heapStart, heapEnd;    // free memory
  uint32_t startMs, lastMs;       // (working values)
};
extern OtaLog otaLog;
extern bool otaLogValid;
