// Alarms, the countdown timer and the forecast: the logic as plain C++ (no Arduino), so
// wfd-control/tests can compile it on the PC and check it. main.cpp does the I/O around it.
#pragma once
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

namespace extras {

static const int N_ALARMS = 4;
// Weekday bits: bit 0 = Monday .. bit 6 = Sunday (the clock's DAY_ABBR order).
static const char *const DAY_KEYS[7] = {"mon", "tue", "wed", "thu", "fri", "sat", "sun"};

struct Alarm {
  uint8_t on, hh, mm, days;
};

// The alarm due at this minute (wday 0 = Monday), or -1.
inline int alarmDue(const Alarm *a, int n, int wday, int hh, int mm) {
  for (int i = 0; i < n; i++)
    if (a[i].on && a[i].hh == hh && a[i].mm == mm && (a[i].days >> wday & 1)) return i;
  return -1;
}

// "H:MM" / "HH:MM" -> hh, mm. False if it isn't a valid time.
inline bool parseTime(const char *s, int *hh, int *mm) {
  if (!s) return false;
  int h = 0, m = 0, n = 0;
  if (sscanf(s, "%d:%d%n", &h, &m, &n) != 2 || s[n] != 0 || h < 0 || h > 23 || m < 0 || m > 59) return false;
  if (strlen(s) < 4 || strlen(s) > 5 || s[strlen(s) - 3] != ':') return false;   // M:SS-style typos
  *hh = h;
  *mm = m;
  return true;
}

// Time left on the 6-character message line: "  4:59", " 59:59", " 1H05M", "23H59M".
inline void timerText(uint32_t secs, char out[8]) {
  if (secs > 86399) secs = 86399;
  if (secs >= 3600) snprintf(out, 8, "%uH%02uM", (unsigned)(secs / 3600), (unsigned)(secs / 60 % 60));
  else snprintf(out, 8, "%2u:%02u", (unsigned)(secs / 60), (unsigned)(secs % 60));
  char pad[8];
  snprintf(pad, sizeof pad, "%6s", out);       // right-align in 6 characters
  memcpy(out, pad, 8);
}

// The forecast on the message line, item 0..2: today's high and low, then tomorrow's high:
// "HI 78F", "LO 55F", "THU 81". The unit is dropped when it wouldn't fit (3-digit temperatures).
inline void forecastText(int item, int hi, int lo, int tmrwHi, int tmrwDay, char unit, char out[8]) {
  if (item == 2) {
    static const char *const ABBR[7] = {"MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"};
    snprintf(out, 8, "%s %d", ABBR[((tmrwDay % 7) + 7) % 7], tmrwHi);
    return;
  }
  int t = item == 0 ? hi : lo;
  const char *lbl = item == 0 ? "HI" : "LO";
  snprintf(out, 8, "%s %d%c", lbl, t, unit);
  if (strlen(out) > 6) snprintf(out, 8, "%s %d", lbl, t);
}

// Read up to `n` numbers from the JSON array `"key":[...]` inside `json` (from `from` on).
// Returns how many were read. Plain text scanning: the firmware gets the reply as a string.
inline int jsonArray(const char *json, const char *from, const char *key, double *out, int n) {
  char pat[48];
  snprintf(pat, sizeof pat, "\"%s\":[", key);
  const char *p = strstr(from ? from : json, pat);
  if (!p) return 0;
  p += strlen(pat);
  int k = 0;
  while (k < n && *p && *p != ']') {
    char *end;
    double v = strtod(p, &end);
    if (end == p) break;                      // null or junk: stop
    out[k++] = v;
    p = end;
    while (*p == ',' || *p == ' ') p++;
  }
  return k;
}

}  // namespace extras
