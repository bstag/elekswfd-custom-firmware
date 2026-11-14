// The microphone: sampled on core 0 by micTask, plus the MIC / MICLOG diagnostics.
#include <Arduino.h>
#include <Update.h>
#include "clock_state.h"
#include "mic_dsp.h"
#include "microphone.h"

static mic::Dsp micDsp;               // runs in micTask on core 0

// ---- microphone: sampled on core 0 while the app is off and something uses it ----
void micTask(void *) {
  static uint16_t buf[mic::N];
  const uint32_t period = 1000000UL / mic::RATE;
  for (;;) {
    // Pause during an OTA update: this task busy-waits on core 0, which the WiFi stack needs then.
    bool want = !appTime.within(APP_ON_MS) && (micAuto || (gaAuto && grid::usesMic(gaAnim))) && !Update.isRunning();
    if (!want || micPause) {
      micBass = micTreble = micLevel = 0;
      for (int b = 0; b < mic::NBANDS; b++) micBands[b] = 0;
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    uint32_t next = micros();
    for (int i = 0; i < mic::N; i++) {          // 256 samples at 8 kHz = 32 ms
      while ((int32_t)(micros() - next) < 0) {}
      buf[i] = analogRead(MIC_PIN);
      next += period;
    }
    micDsp.process(buf);
    micBass = micDsp.colBass;
    micTreble = micDsp.colTreble;
    micLevel = (int)(micDsp.level * 100);
    for (int b = 0; b < mic::NBANDS; b++) micBands[b] = micDsp.bands[b];
    micFrameAt = millis();
    if ((int32_t)(micTraceUntil - micFrameAt) > 0)
      Serial.printf("T %lu %d %d %d %d %d %d\n", (unsigned long)micFrameAt, (int)micDsp.rms, (int)micDsp.lowAmp,
                    (int)micDsp.highAmp, micLevel, micBass, micTreble);
    vTaskDelay(1);                              // let core 0's idle task run
  }
}

// ---- microphone diagnostics ----
// Capture n samples at ~rate Hz with analogRead and print what the signal looks like.
void micCapture(int n, int rate) {
  static uint16_t buf[2048];
  n = constrain(n, 16, 2048);
  uint32_t period = 1000000UL / rate, t0 = micros(), next = t0;
  for (int i = 0; i < n; i++) {
    while ((int32_t)(micros() - next) < 0) {}
    buf[i] = analogRead(MIC_PIN);
    next += period;
  }
  uint32_t us = micros() - t0;
  double mean = 0;
  int lo = 4095, hi = 0;
  for (int i = 0; i < n; i++) { mean += buf[i]; lo = min(lo, (int)buf[i]); hi = max(hi, (int)buf[i]); }
  mean /= n;
  double ac = 0;
  int zc = 0;
  for (int i = 0; i < n; i++) {
    double d = buf[i] - mean;
    ac += d * d;
    if (i && ((buf[i] >= mean) != (buf[i - 1] >= mean))) zc++;
  }
  int hist[9] = {0};          // 0, 1-3, 4-15, 16-63, 64-255, 256-1023, 1k-2k, 2k-3k, 3k+
  for (int i = 0; i < n; i++) {
    int v = buf[i];
    hist[v == 0 ? 0 : v < 4 ? 1 : v < 16 ? 2 : v < 64 ? 3 : v < 256 ? 4 : v < 1024 ? 5 : v < 2048 ? 6 : v < 3072 ? 7 : 8]++;
  }
  Serial.printf("mic n=%d rate=%dHz min=%d max=%d mean=%d acrms=%d zero-cross/s=%d\n", n,
                (int)(n * 1e6 / us), lo, hi, (int)mean, (int)sqrt(ac / n), (int)(zc * 1e6 / us));
  Serial.printf("  hist 0:%d 1-3:%d 4-15:%d 16-63:%d 64-255:%d 256-1023:%d 1k-2k:%d 2k-3k:%d 3k+:%d\n",
                hist[0], hist[1], hist[2], hist[3], hist[4], hist[5], hist[6], hist[7], hist[8]);
  Serial.print("  first 64:");
  for (int i = 0; i < 64; i++) Serial.printf(" %d", buf[i]);
  Serial.println();
}

// Every 100 ms: 400 samples (~10 kHz for 40 ms) -> min/max/mean and a peak-to-peak bar.
void micLog(int secs) {
  for (int n = 0; n < secs * 10; n++) {
    uint32_t t0 = millis();
    int lo = 4095, hi = 0;
    long sum = 0;
    for (int i = 0; i < 400; i++) {
      int v = analogRead(MIC_PIN);
      lo = min(lo, v); hi = max(hi, v); sum += v;
      delayMicroseconds(80);
    }
    int pp = hi - lo, w = constrain(pp / 25, 0, 40);
    char bar[41];
    for (int i = 0; i < 40; i++) bar[i] = i < w ? '#' : '.';
    bar[40] = 0;
    Serial.printf("%3d.%ds min %4d max %4d mean %4ld pp %4d %s\n", n / 10, n % 10, lo, hi, sum / 400, pp, bar);
    while (millis() - t0 < 100) delay(1);
  }
}
