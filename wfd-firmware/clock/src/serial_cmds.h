// The serial command set (USB, and the display commands the app sends over UDP).
#pragma once
#include <Arduino.h>

void handle(const String &s);         // one command line, without the newline
