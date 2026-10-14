// 7x7 grid animations: a C++ port of wfd-control/grid_anim.py (all but "cpu", which needs the
// PC; "audio" and "mouth" take their 7 bands from the clock's own mic), plus "weather" (pixel
// scenes for the current weather), which the Rust app (wfd-app/src/grid_anim.rs) draws the same
// way. Plain C++ with no Arduino dependencies, so wfd-control/tests can compile it on the PC and
// check it frame-for-frame against the Python engine and the Rust app's weather frames.
//
// Rows are bytes: bit c of rows[r] = cell (r, c). Rounding uses rint() (round half to even),
// which matches Python's round() for the deterministic animations.
#pragma once
#include <math.h>
#include <stdint.h>
#include <string.h>

namespace grid {

// Ids are stored in NVS ("gaid"): only ever append.
enum Anim : uint8_t { CUSTOM = 0, SPIN, BOUNCE, RAIN, WAVE, PULSE, SPARKLE, SCAN, LIFE, HEART, AUDIO, MOUTH,
                      WEATHER, N_ANIM };
// Names match grid_anim.py (weather: the Rust app); CUSTOM = frames uploaded to flash (drawn in the app).
static const char *const NAMES[N_ANIM] = {"custom", "spin", "bounce", "rain", "wave", "pulse",
                                          "sparkle", "scan", "life", "heart", "audio", "mouth", "weather"};
inline bool usesMic(int a) { return a == AUDIO || a == MOUTH; }

inline int animFromName(const char *s) {
  for (int i = 0; i < N_ANIM; i++)
    if (strcmp(s, NAMES[i]) == 0) return i;
  return -1;
}

// Weather scenes for WEATHER. Names are the "weather_scene" setting values (plus "auto").
enum Scene : uint8_t { SC_CLEAR = 0, SC_PARTLY, SC_CLOUDY, SC_FOG, SC_RAIN, SC_SNOW, SC_STORM, SC_NIGHT,
                       SC_NIGHT_PARTLY, N_SCENE };
static const char *const SCENE_NAMES[N_SCENE] = {"clear", "partly", "cloudy", "fog", "rain",
                                                 "snow", "storm", "night", "night_partly"};

inline int sceneFromName(const char *s) {
  for (int i = 0; i < N_SCENE; i++)
    if (strcmp(s, SCENE_NAMES[i]) == 0) return i;
  return -1;
}

// Open-Meteo WMO weather code + day/night -> scene. The moon shows on nights you'd see it.
inline int sceneFromWmo(int code, bool day) {
  if (code <= 1) return day ? SC_CLEAR : SC_NIGHT;
  if (code == 2) return day ? SC_PARTLY : SC_NIGHT_PARTLY;
  if (code == 45 || code == 48) return SC_FOG;
  if ((code >= 51 && code <= 67) || (code >= 80 && code <= 82)) return SC_RAIN;
  if ((code >= 71 && code <= 77) || code == 85 || code == 86) return SC_SNOW;
  if (code >= 95) return SC_STORM;
  return SC_CLOUDY;                           // 3 overcast, and anything unknown
}

typedef uint32_t (*RandFn)();                 // random source: esp_random on the clock

static const uint8_t RING[24][2] = {          // border, clockwise from top-left
    {0, 0}, {0, 1}, {0, 2}, {0, 3}, {0, 4}, {0, 5}, {0, 6}, {1, 6}, {2, 6}, {3, 6}, {4, 6}, {5, 6},
    {6, 6}, {6, 5}, {6, 4}, {6, 3}, {6, 2}, {6, 1}, {6, 0}, {5, 0}, {4, 0}, {3, 0}, {2, 0}, {1, 0}};
static const uint8_t HEART_BIG[7] = {0x00, 0x36, 0x7F, 0x7F, 0x3E, 0x1C, 0x08};
static const uint8_t HEART_SMALL[7] = {0x00, 0x00, 0x14, 0x1C, 0x08, 0x00, 0x00};

// Weather art: bit c = column c.
static const uint8_t WX_CLOUD[3] = {0x0C, 0x3E, 0x7F};                         // ..XX... / .XXXXX. / XXXXXXX
static const uint8_t WX_MOON[7] = {0x1C, 0x06, 0x03, 0x03, 0x03, 0x06, 0x1C};  // a crescent opening right
static const int8_t WX_RAYS[8][2] = {{0, 3}, {1, 5}, {3, 6}, {5, 5}, {6, 3}, {5, 1}, {3, 0}, {1, 1}};
static const int8_t WX_DROPS[5][2] = {{3, 0}, {1, 2}, {0, 4}, {2, 6}, {3, 3}};      // {phase, column}
static const int8_t WX_FLAKES[6][2] = {{0, 1}, {2, 4}, {4, 0}, {1, 6}, {5, 3}, {3, 2}};
static const int8_t WX_STARS[5][2] = {{0, 5}, {2, 6}, {5, 5}, {6, 3}, {1, 3}};      // {row, column}
static const int8_t WX_BOLT[5][2] = {{3, 4}, {4, 3}, {4, 4}, {5, 3}, {6, 2}};

struct Engine {
  RandFn rnd;
  int anim = -1;
  uint32_t t = 0;
  double ball[2], vel[2];
  uint8_t drops[7];                           // rain: bit r of drops[c] = drop at (r, c)
  float spark[7][7];
  uint8_t life[7];                            // bit c of life[r]
  const double *bands = nullptr;              // audio/mouth input: 7 values 0..1 (null = silence)
  int scene = SC_CLEAR;                       // weather input: a Scene

  explicit Engine(RandFn r) : rnd(r) { reset(); }

  double rand01() { return (rnd() >> 8) * (1.0 / 16777216.0); }
  int rand7() { return (int)(rnd() % 7); }

  void seedLife() {
    for (int r = 0; r < 7; r++) {
      life[r] = 0;
      for (int c = 0; c < 7; c++)
        if (rand01() < 0.32) life[r] |= 1 << c;
    }
  }

  void reset() {
    ball[0] = ball[1] = 3.0;
    vel[0] = 0.55; vel[1] = 0.37;
    memset(drops, 0, sizeof drops);
    memset(spark, 0, sizeof spark);
    seedLife();
  }

  // Advance animation `a` one step and write its 7 row bytes (a = CUSTOM is not handled here).
  void frame(int a, uint8_t rows[7]) {
    if (a != anim) { anim = a; t = 0; reset(); }
    t++;
    memset(rows, 0, 7);
    switch (a) {
      case BOUNCE: bounce(rows); break;
      case RAIN: rain(rows); break;
      case WAVE: wave(rows); break;
      case PULSE: pulse(rows); break;
      case SPARKLE: sparkle(rows); break;
      case SCAN: for (int r = 0; r < 7; r++) rows[r] = 1 << (t % 7); break;
      case LIFE: lifeStep(rows); break;
      case HEART: { int ph = t % 16; memcpy(rows, (ph < 4 || (ph >= 6 && ph < 9)) ? HEART_BIG : HEART_SMALL, 7); break; }
      case AUDIO: audio(rows); break;
      case MOUTH: mouth(rows); break;
      case WEATHER: weather(rows); break;
      default: spin(rows); break;                // SPIN, and the fallback like the Python engine
    }
  }

  void spin(uint8_t *g) {
    for (int k = 0; k < 5; k++) {
      int i = (int)(((int64_t)t - k) % 24 + 24) % 24;
      g[RING[i][0]] |= 1 << RING[i][1];
    }
  }

  void bounce(uint8_t *g) {
    for (int i = 0; i < 2; i++) {
      ball[i] += vel[i];
      if (ball[i] < 0) { ball[i] = 0; vel[i] = fabs(vel[i]); }
      else if (ball[i] > 6) { ball[i] = 6; vel[i] = -fabs(vel[i]); }
    }
    g[(int)rint(ball[0])] |= 1 << (int)rint(ball[1]);
  }

  void rain(uint8_t *g) {
    for (int c = 0; c < 7; c++) drops[c] = (drops[c] << 1) & 0x7F;   // fall one row, drop off the bottom
    if (rand01() < 0.55) drops[rand7()] |= 1;
    for (int c = 0; c < 7; c++)
      for (int r = 0; r < 7; r++)
        if (drops[c] & (1 << r)) g[r] |= 1 << c;
  }

  void wave(uint8_t *g) {
    for (int c = 0; c < 7; c++) {
      int r = (int)rint(3 + 3 * sin(t * 0.35 + c * 0.7));
      g[r < 0 ? 0 : r > 6 ? 6 : r] |= 1 << c;
    }
  }

  void pulse(uint8_t *g) {
    int rad = (int)((t / 2) % 4);
    for (int r = 0; r < 7; r++)
      for (int c = 0; c < 7; c++) {
        int d = abs(r - 3) > abs(c - 3) ? abs(r - 3) : abs(c - 3);
        if (d == rad) g[r] |= 1 << c;
      }
  }

  void sparkle(uint8_t *g) {
    for (int r = 0; r < 7; r++)
      for (int c = 0; c < 7; c++) spark[r][c] *= 0.78f;
    for (int k = 0; k < 2; k++) { int r = rand7(); spark[r][rand7()] = 1.0f; }
    for (int r = 0; r < 7; r++)
      for (int c = 0; c < 7; c++)
        if (spark[r][c] > 0.3f) g[r] |= 1 << c;
  }

  double band(int c) const { return bands ? bands[c] : 0.0; }

  void audio(uint8_t *g) {                    // 7 bars, bottom-up
    for (int c = 0; c < 7; c++) {
      int h = (int)rint(band(c) * 7);
      for (int r = 0; r < 7; r++)
        if (6 - r < h) g[r] |= 1 << c;
    }
  }

  void mouth(uint8_t *g) {                    // opens with the loudest band
    double m = 0;
    for (int c = 0; c < 7; c++) if (band(c) > m) m = band(c);
    int op = (int)rint((m < 1 ? m : 1) * 3);
    for (int c = 0; c < 7; c++) {
      if (op == 0) { g[3] |= 1 << c; continue; }
      int edge = (c == 0 || c == 6) ? 1 : 0;
      int top = 3 - op + edge, bot = 3 + op - edge;
      g[top < 0 ? 0 : top] |= 1 << c;
      g[bot > 6 ? 6 : bot] |= 1 << c;
    }
  }

  // ---- weather scenes (deterministic: frame t and the scene only) ----
  static void dot(uint8_t *g, int r, int c) {
    if (r >= 0 && r < 7 && c >= 0 && c < 7) g[r] |= 1 << c;
  }
  // Draw `n` art rows at (r0, c0); with wrap > 0 the art scrolls around a strip `wrap` wide.
  static void stamp(uint8_t *g, const uint8_t *art, int n, int r0, int c0, int wrap = 0) {
    for (int r = 0; r < n; r++)
      for (int c = 0; c < 7; c++) {
        if (!(art[r] & (1 << c))) continue;
        int cc = c0 + c;
        if (wrap) cc = ((cc % wrap) + wrap) % wrap;
        dot(g, r0 + r, cc);
      }
  }
  static void sunDisc(uint8_t *g, int r, int c) {
    dot(g, r, c); dot(g, r - 1, c); dot(g, r + 1, c); dot(g, r, c - 1); dot(g, r, c + 1);
  }
  void rainDrops(uint8_t *g, int n) {
    for (int i = 0; i < n; i++) dot(g, 3 + (int)((t + WX_DROPS[i][0]) % 4), WX_DROPS[i][1]);
  }
  void stars(uint8_t *g, int n) {
    for (int i = 0; i < n; i++)
      if ((t + i * 3) % 7 < 4) dot(g, WX_STARS[i][0], WX_STARS[i][1]);
  }

  void weather(uint8_t *g) {
    int s = (int)(t / 2);                     // slow movers step every other frame
    switch (scene) {
      case SC_PARTLY: {                         // sun upper left, a cloud drifting past below
        static const int8_t RAYS[4][2] = {{0, 2}, {2, 0}, {0, 0}, {0, 4}};
        sunDisc(g, 2, 2);
        dot(g, RAYS[s % 4][0], RAYS[s % 4][1]);
        stamp(g, WX_CLOUD, 3, 4, s % 14 - 3, 14);
        break;
      }
      case SC_CLOUDY: {                         // two layers of clouds scrolling left
        int x = -(s % 14);
        stamp(g, WX_CLOUD, 3, 0, x + 1, 14); stamp(g, WX_CLOUD, 3, 0, x + 8, 14);
        stamp(g, WX_CLOUD, 3, 3, x + 5, 14); stamp(g, WX_CLOUD, 3, 3, x + 12, 14);
        break;
      }
      case SC_FOG:                              // dashed bands sliding against each other
        for (int i = 0; i < 3; i++)
          for (int c = 0; c < 7; c++) {
            int p = (i % 2) ? c + s : c - s;
            if (((p % 4) + 4) % 4 < 3) dot(g, 1 + 2 * i, c);
          }
        break;
      case SC_RAIN:
        stamp(g, WX_CLOUD, 3, 0, 0);
        rainDrops(g, 5);
        break;
      case SC_SNOW:                             // flakes falling slowly, drifting side to side
        for (int i = 0; i < 6; i++) {
          int r = (s + WX_FLAKES[i][0]) % 7;
          int c = (WX_FLAKES[i][1] + ((s + i) % 4 < 2 ? 0 : 1)) % 7;
          dot(g, r, c);
        }
        break;
      case SC_STORM: {                          // bolt, rain, and a flash of the whole grid
        int p = (int)(t % 12);
        if (p == 0 || p == 2) { memset(g, 0x7F, 7); break; }
        stamp(g, WX_CLOUD, 3, 0, 0);
        if (p < 6) for (int i = 0; i < 5; i++) dot(g, WX_BOLT[i][0], WX_BOLT[i][1]);
        else rainDrops(g, 3);
        break;
      }
      case SC_NIGHT:
        stamp(g, WX_MOON, 7, 0, 0);
        stars(g, 5);
        break;
      case SC_NIGHT_PARTLY:
        stamp(g, WX_MOON, 7, 0, 0);
        stars(g, 2);
        stamp(g, WX_CLOUD, 3, 4, s % 14 - 3, 14);
        break;
      default:                                  // SC_CLEAR: the sun, rays turning
        sunDisc(g, 3, 3);
        for (int k = 0; k < 8; k += 2) {
          const int8_t *ray = WX_RAYS[(k + t) % 8];
          dot(g, ray[0], ray[1]);
        }
        break;
    }
  }

  void lifeStep(uint8_t *g) {
    if (t % 3 == 0) {
      uint8_t nxt[7] = {0};
      int alive = 0;
      for (int r = 0; r < 7; r++)
        for (int c = 0; c < 7; c++) {
          int n = 0;
          for (int dr = -1; dr <= 1; dr++)
            for (int dc = -1; dc <= 1; dc++)
              if ((dr || dc) && (life[(r + dr + 7) % 7] & (1 << ((c + dc + 7) % 7)))) n++;
          bool on = n == 3 || ((life[r] & (1 << c)) && n == 2);
          if (on) { nxt[r] |= 1 << c; alive++; }
        }
      if (alive == 0 || t % 90 == 0) seedLife();
      else memcpy(life, nxt, 7);
    }
    memcpy(g, life, 7);
  }
};

}  // namespace grid
