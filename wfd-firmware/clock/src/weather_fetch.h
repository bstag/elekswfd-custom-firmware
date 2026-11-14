// The clock's own weather (Open-Meteo, in a background task) and the US ZIP lookup.
#pragma once
#include <Arduino.h>

bool setWeatherZip(const String &zip);   // look up and save the location; false if it failed
bool netWeatherFetch();               // start a fetch; false if it can't start now
void weatherApply();                  // in loop(): take over a finished fetch
