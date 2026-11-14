// The three buttons, and the alert an alarm or the countdown timer raises.
#include <Arduino.h>
#include <WiFi.h>
#include "clock_state.h"
#include "clock_display.h"
#include "wifi_link.h"
#include "buttons_alerts.h"

static uint32_t alertAt = 0;
static const uint32_t ALERT_MAX_MS = 300000;   // an unanswered alert stops after 5 minutes

void startAlert(int kind) {
  alertKind = kind;
  alertAt = millis();
  dirty = true;
  Serial.printf("ALERT %s\n", kind == ALERT_TIMER ? "timer" : "alarm");
}
void stopAlert() {
  if (!alertKind) return;
  alertKind = ALERT_NONE;
  dirty = true;
  Serial.println("ALERT off");
}
// Called 5x a second: fire alarms (once per matching minute), end the timer, time out an alert.
// Returns true while something on the display moves (the timer, the flashing).
bool extrasTick() {
  static int lastMinute = -1;
  if (cdEnd && (int32_t)(millis() - cdEnd) >= 0) { cdEnd = 0; startAlert(ALERT_TIMER); }
  if (alertKind && millis() - alertAt > ALERT_MAX_MS) stopAlert();
  int hh = bcd2dec(curT[2] & 0x3F), mi = bcd2dec(curT[1]), wd = constrain((curT[3] & 7) - 1, 0, 6);
  int minute = wd * 1440 + hh * 60 + mi;
  if (minute != lastMinute) {
    lastMinute = minute;
    if (!alertKind && extras::alarmDue(alarms, extras::N_ALARMS, wd, hh, mi) >= 0) startAlert(ALERT_ALARM);
  }
  return cdEnd || alertKind;
}

// Any button held for 2 s: where is the clock on the network? On WiFi it scrolls its address;
// off WiFi it opens the "WFD-Setup" network so a phone can pick a network again.
static void wifiButton() {
  if (apMode) {
    showInfo("JOIN WFD-SETUP WIFI", 60000);
  } else if (WiFi.status() == WL_CONNECTED) {
    showInfo("IP " + WiFi.localIP().toString(), 20000);
  } else {
    startAP(true);
    showInfo("JOIN WFD-SETUP WIFI", 60000);
  }
  Serial.printf("button held -> %s\n", infoText.c_str());
}

void checkButtons() {
  static uint8_t prev[3] = {1, 1, 1};
  static uint32_t edgeAt[3] = {0, 0, 0};
  static bool used[3] = {false, false, false};   // this press already did something
  const int pins[3] = {25, 26, 27};
  const uint32_t HOLD_MS = 2000;
  for (int i = 0; i < 3; i++) {
    uint8_t v = digitalRead(pins[i]);
    if (v != prev[i] && millis() - edgeAt[i] > 40) {   // debounced edge
      prev[i] = v;
      edgeAt[i] = millis();
      if (v == 0) {                                    // pressed: any button stops an alert
        used[i] = false;
        if (alertKind) { stopAlert(); draw(); used[i] = true; }
      } else if (!used[i]) {                           // released after a short press
        if (i == 0) setStyle(specStyle <= 1 ? N_STYLES : specStyle - 1);   // left: prev
        else setStyle(specStyle >= N_STYLES ? 1 : specStyle + 1);          // middle/right: next
        Serial.printf("button %d -> style %d\n", i, specStyle);
        draw();
      }
    } else if (prev[i] == 0 && !used[i] && millis() - edgeAt[i] >= HOLD_MS) {
      used[i] = true;
      wifiButton();
    }
  }
}
