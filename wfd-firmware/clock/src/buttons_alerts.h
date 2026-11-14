// The three buttons, and the alert raised by an alarm or the countdown timer.
#pragma once

void startAlert(int kind);            // ALERT_ALARM / ALERT_TIMER
void stopAlert();
bool extrasTick();                    // 5x a second from loop(); true while the display moves
void checkButtons();                  // every loop() pass
