// The three buttons, and the alert an alarm or the countdown timer raises.
#include <Arduino.h>
#include <WiFi.h>
#include "clock_state.h"
#include "clock_display.h"
#include "wifi_link.h"
#include "buttons_alerts.h"

Alerts alerts;

static uint32_t alertAt = 0;
static const uint32_t ALERT_MAX_MS = 300000;   // an unanswered alert stops after 5 minutes

void startAlert(int kind) {
  alerts.kind = kind;
  alertAt = millis();
  disp.dirty = true;
  Serial.printf("ALERT %s\n", kind == ALERT_TIMER ? "timer" : "alarm");
}
void stopAlert() {
  if (!alerts.kind) return;
  alerts.kind = ALERT_NONE;
  disp.dirty = true;
  Serial.println("ALERT off");
}
// Called 5x a second: fire alarms (once per matching minute), end the timer, time out an alert.
// Returns true while something on the display moves (the timer, the flashing).
bool extrasTick() {
  static int lastMinute = -1;
  if (alerts.cdEnd && (int32_t)(millis() - alerts.cdEnd) >= 0) { alerts.cdEnd = 0; startAlert(ALERT_TIMER); }
  if (alerts.kind && millis() - alertAt > ALERT_MAX_MS) stopAlert();
  int hh = bcd2dec(disp.now[2] & 0x3F), mi = bcd2dec(disp.now[1]), wd = constrain((disp.now[3] & 7) - 1, 0, 6);
  int minute = wd * 1440 + hh * 60 + mi;
  if (minute != lastMinute) {
    lastMinute = minute;
    if (!alerts.kind && extras::alarmDue(alerts.alarms, extras::N_ALARMS, wd, hh, mi) >= 0) startAlert(ALERT_ALARM);
  }
  return alerts.cdEnd || alerts.kind;
}

// Any button held for 2 s: where is the clock on the network? On WiFi it scrolls its address;
// off WiFi it opens the "WFD-Setup" network so a phone can pick a network again.
static void wifiButton() {
  if (net.apMode) {
    showInfo("JOIN WFD-SETUP WIFI", 60000);
  } else if (WiFi.status() == WL_CONNECTED) {
    showInfo("IP " + WiFi.localIP().toString(), 20000);
  } else {
    startAP(true);
    showInfo("JOIN WFD-SETUP WIFI", 60000);
  }
  Serial.printf("button held -> %s\n", disp.infoText.c_str());
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
        if (alerts.kind) { stopAlert(); draw(); used[i] = true; }
      } else if (!used[i]) {                           // released after a short press
        if (i == 0) setStyle(disp.style <= 1 ? N_STYLES : disp.style - 1);   // left: prev
        else setStyle(disp.style >= N_STYLES ? 1 : disp.style + 1);          // middle/right: next
        Serial.printf("button %d -> style %d\n", i, disp.style);
        draw();
      }
    } else if (prev[i] == 0 && !used[i] && millis() - edgeAt[i] >= HOLD_MS) {
      used[i] = true;
      wifiButton();
    }
  }
}
