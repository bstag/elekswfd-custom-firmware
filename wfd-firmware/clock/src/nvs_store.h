// Everything the clock keeps in flash (NVS namespace "wfd"): one table in nvs_store.cpp gives
// each saved value its key, type and default, and is used both to load them at boot and to save
// one. Clocks in the field depend on these keys, types and defaults: don't change them.
#pragma once
#include <Arduino.h>

// In load order (gn and ganim before gaid: gaid's default depends on whether a drawing loaded).
enum Pref : uint8_t {
  P_STYLE, P_BRIGHT, P_CLK24,                                  // display
  P_GN, P_GFPS, P_GFLAG, P_GAUTO, P_GANIM, P_GAID,             // grid when the app is off
  P_WEBPASS,                                                   // web password
  P_WXSC,                                                      // weather_scene
  P_ALARMS,                                                    // alarms
  P_OTALOG,                                                    // the last upload's log
  P_MICAUTO, P_MICSPIN,                                        // mic when the app is off, and how it is sampled
  P_OMAUTO, P_MMODE, P_MROT, P_MDWELL, P_MSCROLL, P_OMTEXT,    // message line when the app is off
  P_TZ, P_LAT, P_LON, P_WUNIT,                                 // time zone, weather location and units
  P_ZIP, P_SSID, P_PASS,                                       // only in flash (read when needed)
  N_PREFS
};

void prefsLoad();                     // setup(): open the namespace and load every value into RAM
void prefSave(Pref p);                // save a value's current RAM copy
String prefString(Pref p);            // a string kept only in flash (P_ZIP, P_SSID, P_PASS)
void prefPutString(Pref p, const String &v);
void prefRemove(Pref p);
