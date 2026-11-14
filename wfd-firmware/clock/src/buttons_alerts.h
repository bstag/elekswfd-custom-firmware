// The three buttons, and the alert raised by an alarm or the countdown timer, with their state.
#pragma once
#include <Arduino.h>
#include "extras.h"

enum { ALERT_NONE = 0, ALERT_ALARM, ALERT_TIMER };
struct Alerts {
  extras::Alarm alarms[extras::N_ALARMS] = {};   // saved (NVS "alarms")
  uint32_t cdEnd = 0;                 // millis() when the timer ends, 0 = no timer
  int kind = ALERT_NONE;              // the alert going off now
};
extern Alerts alerts;
inline uint32_t timerLeft() {
  return alerts.cdEnd && (int32_t)(alerts.cdEnd - millis()) > 0 ? (alerts.cdEnd - millis() + 999) / 1000 : 0;
}

void startAlert(int kind);            // ALERT_ALARM / ALERT_TIMER
void stopAlert();
bool extrasTick();                    // 5x a second from loop(); true while the display moves
void checkButtons();                  // every loop() pass
