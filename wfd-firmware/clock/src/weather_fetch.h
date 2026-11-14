// The clock's own weather (Open-Meteo, in a background task) and the US ZIP lookup, and the
// weather state: the current reading (from the app's WX or our own fetch) and the forecast.
#pragma once
#include <Arduino.h>
#include "clock_state.h"

struct Weather {
  String lat, lon;                    // location (saved; empty = none)
  int units = 0;                      // setting: 0=F 1=C (saved)
  uint32_t lastFetch = 0;
  int code = 0, temp = 0, unit = 0;   // code 0=none 1=sunny 2=cloud 3=rains; unit 0=F 1=C
  Stamp time;                         // last weather (WX from the PC or our own fetch)
  int scene = -1;                     // grid::Scene from our own fetch (WMO code + day/night), -1 = none yet
  uint32_t sceneTime = 0;
  int scenePick = -1;                 // "weather_scene" setting: a grid::Scene, or -1 = auto (saved)
  // forecast from our own fetch: day 0 = today (Open-Meteo daily, timezone=auto)
  int fcN = 0, fcHi[4] = {0}, fcLo[4] = {0}, fcCode[4] = {0};
  uint32_t fcTime = 0;
};
extern Weather wx;
inline bool wxFresh() { return wx.time.within(1200000); }   // PC every 60 s, own fetch every 15 min
inline bool fcFresh() { return wx.fcN >= 2 && millis() - wx.fcTime < 10800000; }

bool setWeatherZip(const String &zip);   // look up and save the location; false if it failed
bool netWeatherFetch();               // start a fetch; false if it can't start now
void weatherApply();                  // in loop(): take over a finished fetch
