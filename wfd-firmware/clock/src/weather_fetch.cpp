// The clock's own weather (Open-Meteo, fetched by a background task) and the US ZIP lookup.
#include <Arduino.h>
#include <WiFi.h>
#include <HTTPClient.h>
#include <WiFiClientSecure.h>
#include <Update.h>
#include <ArduinoJson.h>
#include "clock_state.h"
#include "nvs_store.h"
#include "weather_fetch.h"

Weather wx;

bool setWeatherZip(const String &zip) {
  if (WiFi.status() != WL_CONNECTED) return false;
  WiFiClient cli;
  HTTPClient http;
  http.setTimeout(6000);
  if (!http.begin(cli, "http://api.zippopotam.us/us/" + zip)) return false;
  bool ok = false;
  if (http.GET() == 200) {
    // {"post code": "76708", ..., "places": [{"place name": "Waco", "longitude": "-97.2", "latitude": "31.5", ...}]}
    JsonDocument filter, doc;
    filter["places"][0]["latitude"] = true;      // (an array filter's first element covers every element)
    filter["places"][0]["longitude"] = true;
    const char *la = nullptr, *lo = nullptr;
    if (!deserializeJson(doc, http.getString(), DeserializationOption::Filter(filter))) {
      la = doc["places"][0]["latitude"];
      lo = doc["places"][0]["longitude"];
    }
    if (la && lo) {                                // both present, as strings
      wx.lat = la;
      wx.lon = lo;
      prefSave(P_LAT); prefSave(P_LON); prefPutString(P_ZIP, zip);
      wx.lastFetch = 0;
      ok = true;
    }
  }
  http.end();
  return ok;
}

// ---- weather: fetched by a background task, so the display and the network keep running ----
// The HTTPS request takes seconds (TLS handshake). loop() only hands the task a job
// (netWeatherFetch) and picks up the finished result (weatherApply); the task never touches
// the display state itself.
struct WxJob { char lat[24], lon[24]; int unit; };
struct WxResult { bool ok; int code, scene, temp, unit; int n, hi[4], lo[4], wc[4]; };
static WxJob wxJob;
static WxResult wxResult;
static volatile bool wxBusy = false, wxDone = false;
static TaskHandle_t wxTaskHandle = nullptr;

// The leading numbers of a JSON array, up to n (stops at a null or anything else). 0 if not an array.
static int numbersOf(JsonVariantConst a, double *out, int n) {
  int k = 0;
  for (JsonVariantConst v : a.as<JsonArrayConst>()) {
    if (k >= n || !v.is<double>()) break;
    out[k++] = v.as<double>();
  }
  return k;
}

static void weatherFetch(const WxJob &job, WxResult &r) {   // Open-Meteo directly over HTTPS
  r.ok = false;
  r.n = 0;
  WiFiClientSecure cli;
  cli.setInsecure();
  HTTPClient http;
  http.setTimeout(6000);
  String url = String("https://api.open-meteo.com/v1/forecast?latitude=") + job.lat + "&longitude=" + job.lon +
               "&current=temperature_2m,weather_code,is_day&temperature_unit=" + (job.unit ? "celsius" : "fahrenheit") +
               "&daily=weather_code,temperature_2m_max,temperature_2m_min&forecast_days=4&timezone=auto";
  if (!http.begin(cli, url)) return;
  if (http.GET() == 200) {
    // {"current":{"temperature_2m":71.6,"weather_code":3,"is_day":1,...},
    //  "daily":{"weather_code":[3,61,..],"temperature_2m_max":[..],"temperature_2m_min":[..],...}, ...}
    // The documents keep their data on the heap; the filter drops everything else (units, times).
    JsonDocument filter, doc;
    JsonObject fc = filter["current"].to<JsonObject>();
    fc["temperature_2m"] = true;
    fc["weather_code"] = true;
    fc["is_day"] = true;
    JsonObject fd = filter["daily"].to<JsonObject>();
    fd["weather_code"] = true;
    fd["temperature_2m_max"] = true;
    fd["temperature_2m_min"] = true;
    JsonObjectConst cur;
    if (!deserializeJson(doc, http.getString(), DeserializationOption::Filter(filter))) cur = doc["current"];
    if (cur["temperature_2m"].is<float>() && cur["weather_code"].is<float>()) {   // (is<float>: any number)
      r.temp = (int)lroundf(cur["temperature_2m"].as<float>());
      int code = cur["weather_code"].as<int>();
      r.code = code <= 1 ? 1 : (code == 2 || code == 3 || code == 45 || code == 48) ? 2 : 3;
      r.scene = grid::sceneFromWmo(code, (cur["is_day"] | 1) != 0);   // no is_day: day
      JsonObjectConst daily = doc["daily"];
      double hi[4], lo[4], wc[4];
      int n = numbersOf(daily["temperature_2m_max"], hi, 4);
      n = min(n, numbersOf(daily["temperature_2m_min"], lo, 4));
      n = min(n, numbersOf(daily["weather_code"], wc, 4));
      if (n >= 2) {
        for (int d = 0; d < n; d++) { r.hi[d] = (int)lround(hi[d]); r.lo[d] = (int)lround(lo[d]); r.wc[d] = (int)wc[d]; }
        r.n = n;
      }
      r.unit = job.unit;
      r.ok = true;
    }
  }
  http.end();
}

static void weatherTask(void *) {
  for (;;) {
    ulTaskNotifyTake(pdTRUE, portMAX_DELAY);    // sleeps until netWeatherFetch() has a job
    weatherFetch(wxJob, wxResult);
    wxDone = true;
  }
}

// Ask for a fetch. False if it can't start now (one is running, no WiFi or location, an update
// is being written): the caller tries again on a later pass.
bool netWeatherFetch() {
  if (wxBusy || WiFi.status() != WL_CONNECTED || wx.lat.length() == 0 || Update.isRunning()) return false;
  if (!wxTaskHandle &&       // core 1 with loop(): the mic task keeps core 0's timing to itself
      xTaskCreatePinnedToCore(weatherTask, "weather", 12288, nullptr, 1, &wxTaskHandle, 1) != pdPASS) {
    wxTaskHandle = nullptr;
    return false;
  }
  strlcpy(wxJob.lat, wx.lat.c_str(), sizeof wxJob.lat);
  strlcpy(wxJob.lon, wx.lon.c_str(), sizeof wxJob.lon);
  wxJob.unit = wx.units;
  wxBusy = true;
  xTaskNotifyGive(wxTaskHandle);
  return true;
}

// In loop(): take over a finished fetch.
void weatherApply() {
  if (!wxDone) return;
  const WxResult &r = wxResult;
  if (r.ok) {
    wx.code = r.code;
    wx.scene = r.scene;
    wx.sceneTime = millis();
    if (r.n >= 2) {
      for (int d = 0; d < r.n; d++) { wx.fcHi[d] = r.hi[d]; wx.fcLo[d] = r.lo[d]; wx.fcCode[d] = r.wc[d]; }
      wx.fcN = r.n;
      wx.fcTime = millis();
    }
    wx.temp = r.temp;
    wx.unit = r.unit;
    wx.time.mark();
    disp.dirty = true;
  }
  wxDone = false;
  wxBusy = false;
}
