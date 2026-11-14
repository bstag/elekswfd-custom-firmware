// The clock's microphone: the sampling task (core 0), what it hands over, and the MIC / MICLOG
// diagnostics.
#pragma once
#include <Arduino.h>
#include "mic_dsp.h"

static const int MIC_PIN = 32;        // microphone: GPIO32 / ADC1_CH4, behind an op-amp

// Shared between micTask (core 0) and loop() (core 1).
struct MicState {
  volatile int bass = 0, treble = 0, level = 0;   // latest results
  double bands[mic::NBANDS] = {0};    // grid audio/mouth input
  volatile uint32_t frameAt = 0;      // millis of the last frame
  volatile bool pause = false;        // MIC/MICLOG use the ADC
  volatile uint32_t traceUntil = 0;   // MICTRACE: print every frame
};
extern MicState micState;

void micTask(void *);
void micCapture(int n, int rate);
void micLog(int secs);
