// The microphone: sampled on core 0 by micTask, plus the MIC / MICLOG diagnostics.
#include <Arduino.h>
#include <Update.h>
#include <driver/timer.h>
#include "clock_state.h"
#include "mic_dsp.h"
#include "microphone.h"

MicState micState;
static mic::Dsp micDsp;               // runs in micTask on core 0

// The sample clock: hardware timer group 0 / timer 0 at 1 MHz (APB 80 MHz / 80), an alarm every
// 1/RATE s that wakes micTask, which takes one analogRead per tick and sleeps in between. Its
// interrupt is allocated on core 0 (the timer is set up from micTask), and it runs only while
// a frame is being taken.
static const timer_group_t MIC_TG = TIMER_GROUP_0;
static const timer_idx_t MIC_TIMER = TIMER_0;
static TaskHandle_t micTaskHandle = nullptr;

static bool IRAM_ATTR micTick(void *) {
  BaseType_t woken = pdFALSE;
  vTaskNotifyGiveFromISR(micTaskHandle, &woken);
  return woken == pdTRUE;                       // the driver yields at the end of the ISR
}

static bool micClockBegin() {
  timer_config_t cfg = {};
  cfg.alarm_en = TIMER_ALARM_EN;
  cfg.counter_en = TIMER_PAUSE;
  cfg.intr_type = TIMER_INTR_LEVEL;
  cfg.counter_dir = TIMER_COUNT_UP;
  cfg.auto_reload = TIMER_AUTORELOAD_EN;
  cfg.divider = 80;                             // 1 tick = 1 us
  return timer_init(MIC_TG, MIC_TIMER, &cfg) == ESP_OK &&
         timer_set_counter_value(MIC_TG, MIC_TIMER, 0) == ESP_OK &&
         timer_set_alarm_value(MIC_TG, MIC_TIMER, 1000000UL / mic::RATE) == ESP_OK &&
         timer_enable_intr(MIC_TG, MIC_TIMER) == ESP_OK &&
         timer_isr_callback_add(MIC_TG, MIC_TIMER, micTick, nullptr, 0) == ESP_OK;
}

// ---- microphone: sampled on core 0 while the app is off and something uses it ----
void micTask(void *) {
  static uint16_t buf[mic::N];
  micTaskHandle = xTaskGetCurrentTaskHandle();
  if (!micClockBegin()) {
    Serial.println("mic: sample timer failed; mic off");
    vTaskDelete(nullptr);
  }
  for (;;) {
    // Pause during an OTA update: leave core 0 to the WiFi stack then.
    bool want = !live.appTime.within(APP_ON_MS) && (standalone.micAuto || (standalone.gaAuto && grid::usesMic(standalone.gaAnim))) && !Update.isRunning();
    if (!want || micState.pause) {
      micState.bass = micState.treble = micState.level = 0;
      for (int b = 0; b < mic::NBANDS; b++) micState.bands[b] = 0;
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    // 256 samples at 8 kHz = 32 ms: the first now, then one per timer tick. A tick that arrives
    // while the task can't run (a higher-priority task on core 0) is counted, and taken late.
    ulTaskNotifyTake(pdTRUE, 0);                // no ticks left over from the last frame
    timer_set_counter_value(MIC_TG, MIC_TIMER, 0);
    timer_start(MIC_TG, MIC_TIMER);
    int got = 0;
    buf[got++] = analogRead(MIC_PIN);
    while (got < mic::N && ulTaskNotifyTake(pdFALSE, pdMS_TO_TICKS(50)))
      buf[got++] = analogRead(MIC_PIN);
    timer_pause(MIC_TG, MIC_TIMER);
    if (got < mic::N) continue;                 // no tick for 50 ms (can't happen with a working timer)
    micDsp.process(buf);
    micState.bass = micDsp.colBass;
    micState.treble = micDsp.colTreble;
    micState.level = (int)(micDsp.level * 100);
    for (int b = 0; b < mic::NBANDS; b++) micState.bands[b] = micDsp.bands[b];
    micState.frameAt = millis();
    if ((int32_t)(micState.traceUntil - micState.frameAt) > 0)
      Serial.printf("T %lu %d %d %d %d %d %d\n", (unsigned long)micState.frameAt, (int)micDsp.rms, (int)micDsp.lowAmp,
                    (int)micDsp.highAmp, micState.level, micState.bass, micState.treble);
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
