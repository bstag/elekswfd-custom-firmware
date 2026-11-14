// Firmware upload over the web (POST /update) and its log.
#pragma once
#include <Arduino.h>

// The last OTA upload, for diagnosing stalls: saved to NVS ("otalog") when an upload ends, so it
// survives the restart after a good update and is still there after an aborted one. Shown as
// state.ota in /api/status and printed as an "OTA ..." line on serial.
struct OtaLog {
  char result[56];                // ok | no auth | begin failed: .. | write failed: .. | end failed: .. | aborted
  uint32_t bytes, chunks, ms;     // received, pieces, start -> end
  uint32_t maxGapMs, gapAt;       // longest pause between pieces, and how far in (bytes) it came
  int32_t rssiStart, rssiEnd;     // WiFi signal (dBm)
  uint32_t heapStart, heapEnd;    // free memory
  uint32_t startMs, lastMs;       // (working values)
};
extern OtaLog otaLog;
extern bool otaLogValid;

extern bool otaOk;                    // set by the upload handler, read by the final POST handler
void handleUpdateUpload();
