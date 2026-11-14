// What the clock shows by itself when the app is off: the grid animation (built-in or the stored
// drawing) and the on-device message line engine.
#pragma once
#include <Arduino.h>

void setGridAnim(int anim, int fps, int flag);   // choose it (turns it on) and save it
void storeFrames(const uint8_t *rows, int n, int fps, int flag);   // save a drawing and play it

extern uint32_t msgStepAt;            // last message engine step (0 = step now)
void msgStep();                       // one engine step -> engRam / engTemp / msgShown
void setMsgText(String t);
void setMsgCfg(int mode, int rot, int dwell, int scroll);
int todayIdx();                       // 0 = Monday
