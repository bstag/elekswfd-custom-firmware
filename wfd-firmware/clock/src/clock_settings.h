// Settings and status as JSON: one table of settings shared by POST /api/set, the /control form
// and CFG/CFGSET over USB, plus the state report for /api/status.
#pragma once
#include <Arduino.h>
#include <ArduinoJson.h>

extern const char *MSG_MODE_NAMES[];  // index = SRC_* / MODE_ROTATE

bool applySettings(JsonObjectConst in, JsonDocument &out);   // all keys valid -> apply them all
void serialCfg(const String &json);   // CFG / CFGSET: the settings layer over USB
int currentScene();                   // what the weather grid animation shows now
void configJson(JsonObject c);
void stateJson(JsonObject s);
