// The panel and the RTC: TM1680 (I2C 0x73) and DS3231 (0x68), and draw(), which rebuilds the
// whole display RAM from the shared state and flushes it.
#pragma once
#include <Arduino.h>

void tmCmd(uint8_t c);
void tmInit(uint8_t pwm);
uint8_t bcd2dec(uint8_t v);
bool rtcRead(uint8_t *o);
uint8_t rtcStatus();
void rtcSet(int yy, int mo, int dd, int dow, int hh, int mi, int ss);

void setStyle(int v);                 // spectrum style 1..N_STYLES, saved
void setBrightness(int v);            // 1..16, saved
void render6(const String &s, int start, uint8_t *out);   // 6 chars of s from `start` -> out[12] (14-seg)
void draw();
void showInfo(const String &text, uint32_t ms);           // a notice on the message line for `ms`
