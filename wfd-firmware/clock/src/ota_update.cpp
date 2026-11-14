// Firmware upload over the web (POST /update), with its log saved to NVS.
#include <Arduino.h>
#include <WiFi.h>
#include <Update.h>
#include "clock_state.h"
#include "ota_update.h"

bool otaOk = false;

static void otaResult(const char *what, const char *detail = nullptr) {
  if (strcmp(otaLog.result, "running")) return;   // keep the first failure
  snprintf(otaLog.result, sizeof otaLog.result, detail ? "%s: %s" : "%s", what, detail);
}

static void otaFinish() {
  otaLog.ms = millis() - otaLog.startMs;
  otaLog.rssiEnd = WiFi.RSSI();
  otaLog.heapEnd = ESP.getFreeHeap();
  otaLogValid = true;
  prefs.putBytes("otalog", &otaLog, sizeof otaLog);
  Serial.printf("OTA %s bytes=%u chunks=%u ms=%u maxgap=%ums@%u rssi=%d..%d heap=%u..%u\n", otaLog.result,
                otaLog.bytes, otaLog.chunks, otaLog.ms, otaLog.maxGapMs, otaLog.gapAt, otaLog.rssiStart,
                otaLog.rssiEnd, otaLog.heapStart, otaLog.heapEnd);
}

void handleUpdateUpload() {
  HTTPUpload &up = web.upload();
  if (up.status == UPLOAD_FILE_START) {
    otaOk = false;
    memset(&otaLog, 0, sizeof otaLog);
    strcpy(otaLog.result, "running");
    otaLog.startMs = otaLog.lastMs = millis();
    otaLog.rssiStart = WiFi.RSSI();
    otaLog.heapStart = ESP.getFreeHeap();
    if (webPass.length() && !web.authenticate("admin", webPass.c_str())) {   // POST handler sends the 401
      otaResult("no auth");
      otaFinish();
      return;
    }
    otaOk = Update.begin(UPDATE_SIZE_UNKNOWN);
    if (!otaOk) otaResult("begin failed", Update.errorString());
  } else if (up.status == UPLOAD_FILE_WRITE) {
    uint32_t now = millis(), gap = now - otaLog.lastMs;
    if (gap > otaLog.maxGapMs) { otaLog.maxGapMs = gap; otaLog.gapAt = otaLog.bytes; }
    otaLog.lastMs = now;
    otaLog.bytes += up.currentSize;
    otaLog.chunks++;
    if (otaOk && Update.write(up.buf, up.currentSize) != up.currentSize) {
      otaOk = false;
      otaResult("write failed", Update.errorString());
    }
  } else if (up.status == UPLOAD_FILE_END) {
    if (otaOk) {
      otaOk = Update.end(true);
      if (!otaOk) otaResult("end failed", Update.errorString());
    }
    otaResult("ok");
    otaFinish();
  } else if (up.status == UPLOAD_FILE_ABORTED) {   // the connection dropped or stalled mid-upload
    if (otaOk) Update.abort();
    otaOk = false;
    otaResult("aborted");
    otaFinish();
  }
  if (!otaOk && up.status != UPLOAD_FILE_START && Update.isRunning()) Update.abort();
}
