// The saved values: the one place that knows the NVS namespace, keys, types and defaults.
#include <Preferences.h>
#include "clock_state.h"
#include "buttons_alerts.h"
#include "ota_update.h"
#include "weather_fetch.h"
#include "nvs_store.h"

static Preferences prefs;

namespace {

struct PrefSpec {
  Pref id;                            // = its index in PREFS (checked below)
  const char *key;
  char type;                          // 'i' int32, 's' string, 'b' bytes
  void *var;                          // the RAM copy (int / String / bytes); nullptr = only in flash
  int def, lo, hi;                    // 'i': default; clamped to lo..hi when loaded (lo > hi: not clamped)
  int (*defFn)();                     // 'i': the default, when it depends on what loaded before
  const char *sdef;                   // 's': default
  size_t (*len)();                    // 'b': how many bytes (0: nothing to load or save)
  void (*loaded)(bool full);          // 'b': after loading; full = all `len` bytes were there
};

constexpr PrefSpec I(Pref id, const char *key, int *var, int def, int lo = 1, int hi = 0, int (*defFn)() = nullptr) {
  return {id, key, 'i', var, def, lo, hi, defFn, nullptr, nullptr, nullptr};
}
constexpr PrefSpec S(Pref id, const char *key, String *var, const char *def) {
  return {id, key, 's', var, 0, 1, 0, nullptr, def, nullptr, nullptr};
}
constexpr PrefSpec B(Pref id, const char *key, void *var, size_t (*len)(), void (*loaded)(bool)) {
  return {id, key, 'b', var, 0, 1, 0, nullptr, nullptr, len, loaded};
}

// stored frames keep playing after an upgrade from before "gaid"; otherwise a built-in animation
int gaidDefault() { return standalone.gaN > 0 ? grid::CUSTOM : grid::SPIN; }
size_t ganimLen() { return (size_t)standalone.gaN * 7; }
void ganimLoaded(bool full) { if (!full) standalone.gaN = 0; }
size_t alarmsLen() { return sizeof alerts.alarms; }
void alarmsLoaded(bool full) { if (!full) memset(alerts.alarms, 0, sizeof alerts.alarms); }
size_t otalogLen() { return sizeof otaLog; }
void otalogLoaded(bool full) { otaLogValid = full; }

constexpr PrefSpec PREFS[] = {
  I(P_STYLE,   "style",   &disp.style, 4),
  I(P_BRIGHT,  "bright",  &disp.brightness, 6),
  I(P_CLK24,   "clk24",   &disp.clock24, 1),
  I(P_GN,      "gn",      &standalone.gaN, 0, 0, GA_MAX),
  I(P_GFPS,    "gfps",    &standalone.gaFps, 8),
  I(P_GFLAG,   "gflag",   &standalone.gaFlag, 0),
  I(P_GAUTO,   "gauto",   &standalone.gaAuto, 0),
  B(P_GANIM,   "ganim",   standalone.gaFrames, ganimLen, ganimLoaded),
  I(P_GAID,    "gaid",    &standalone.gaAnim, grid::SPIN, 0, grid::N_ANIM - 1, gaidDefault),
  S(P_WEBPASS, "webpass", &net.webPass, ""),
  I(P_WXSC,    "wxsc",    &wx.scenePick, -1, -1, grid::N_SCENE - 1),
  B(P_ALARMS,  "alarms",  alerts.alarms, alarmsLen, alarmsLoaded),
  B(P_OTALOG,  "otalog",  &otaLog, otalogLen, otalogLoaded),
  I(P_MICAUTO, "micauto", &standalone.micAuto, 1),
  I(P_MICSPIN, "micspin", &standalone.micSpin, 0, 0, 1),
  I(P_OMAUTO,  "omauto",  &standalone.omAuto, 0),
  I(P_MMODE,   "mmode",   &standalone.msgMode, SRC_TEXT, 0, N_SRC - 1),
  I(P_MROT,    "mrot",    &standalone.msgRot, 0b111),
  I(P_MDWELL,  "mdwell",  &standalone.msgDwell, 8),
  I(P_MSCROLL, "mscroll", &standalone.msgScroll, 350),
  S(P_OMTEXT,  "omtext",  &standalone.msgText, "WFD CLOCK"),
  S(P_TZ,      "tz",      &net.tz, "CST6CDT,M3.2.0,M11.1.0"),   // US Central w/ DST
  S(P_LAT,     "lat",     &wx.lat, ""),
  S(P_LON,     "lon",     &wx.lon, ""),
  I(P_WUNIT,   "wunit",   &wx.units, 0),
  S(P_ZIP,     "zip",     nullptr, ""),
  S(P_SSID,    "ssid",    nullptr, ""),
  S(P_PASS,    "pass",    nullptr, ""),
};
static_assert(sizeof PREFS / sizeof PREFS[0] == N_PREFS, "one PREFS entry per Pref");
constexpr bool inOrder(int i) { return i == N_PREFS || (PREFS[i].id == i && inOrder(i + 1)); }
static_assert(inOrder(0), "PREFS lists the settings in Pref order");

}  // namespace

void prefsLoad() {
  prefs.begin("wfd", false);
  for (const PrefSpec &s : PREFS) {
    if (!s.var) continue;                         // read from flash when it's needed
    if (s.type == 'i') {
      int v = prefs.getInt(s.key, s.defFn ? s.defFn() : s.def);
      if (s.lo <= s.hi) v = constrain(v, s.lo, s.hi);
      *(int *)s.var = v;
    } else if (s.type == 's') {
      *(String *)s.var = prefs.getString(s.key, s.sdef);
    } else {
      size_t n = s.len();
      if (n) s.loaded(prefs.getBytes(s.key, s.var, n) == n);
    }
  }
  prefs.remove("om"); prefs.remove("omn"); prefs.remove("omfps");   // pre-rendered frames: no longer used
}

void prefSave(Pref p) {
  const PrefSpec &s = PREFS[p];
  if (!s.var) return;
  if (s.type == 'i') prefs.putInt(s.key, *(int *)s.var);
  else if (s.type == 's') prefs.putString(s.key, *(String *)s.var);
  else prefs.putBytes(s.key, s.var, s.len());
}

String prefString(Pref p) { return prefs.getString(PREFS[p].key, PREFS[p].sdef); }
void prefPutString(Pref p, const String &v) { prefs.putString(PREFS[p].key, v); }
void prefRemove(Pref p) { prefs.remove(PREFS[p].key); }
