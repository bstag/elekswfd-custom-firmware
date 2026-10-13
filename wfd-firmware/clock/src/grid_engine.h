// 7x7 grid animations: a C++ port of wfd-control/grid_anim.py (all but "cpu", which needs the
// PC; "audio" and "mouth" take their 7 bands from the clock's own mic). Plain C++ with no Arduino dependencies, so wfd-control/tests can compile it on
// the PC and check it frame-for-frame against the Python engine.
//
// Rows are bytes: bit c of rows[r] = cell (r, c). Rounding uses rint() (round half to even),
// which matches Python's round() for the deterministic animations.
#pragma once
#include <math.h>
#include <stdint.h>
#include <string.h>

namespace grid {

// Ids are stored in NVS ("gaid"): only ever append.
enum Anim : uint8_t { CUSTOM = 0, SPIN, BOUNCE, RAIN, WAVE, PULSE, SPARKLE, SCAN, LIFE, HEART, AUDIO, MOUTH, N_ANIM };
// Names match grid_anim.py; CUSTOM = frames uploaded to flash (drawn in the app).
static const char *const NAMES[N_ANIM] = {"custom", "spin", "bounce", "rain", "wave", "pulse",
                                          "sparkle", "scan", "life", "heart", "audio", "mouth"};
inline bool usesMic(int a) { return a == AUDIO || a == MOUTH; }

inline int animFromName(const char *s) {
  for (int i = 0; i < N_ANIM; i++)
    if (strcmp(s, NAMES[i]) == 0) return i;
  return -1;
}

typedef uint32_t (*RandFn)();                 // random source: esp_random on the clock

static const uint8_t RING[24][2] = {          // border, clockwise from top-left
    {0, 0}, {0, 1}, {0, 2}, {0, 3}, {0, 4}, {0, 5}, {0, 6}, {1, 6}, {2, 6}, {3, 6}, {4, 6}, {5, 6},
    {6, 6}, {6, 5}, {6, 4}, {6, 3}, {6, 2}, {6, 1}, {6, 0}, {5, 0}, {4, 0}, {3, 0}, {2, 0}, {1, 0}};
static const uint8_t HEART_BIG[7] = {0x00, 0x36, 0x7F, 0x7F, 0x3E, 0x1C, 0x08};
static const uint8_t HEART_SMALL[7] = {0x00, 0x00, 0x14, 0x1C, 0x08, 0x00, 0x00};

struct Engine {
  RandFn rnd;
  int anim = -1;
  uint32_t t = 0;
  double ball[2], vel[2];
  uint8_t drops[7];                           // rain: bit r of drops[c] = drop at (r, c)
  float spark[7][7];
  uint8_t life[7];                            // bit c of life[r]
  const double *bands = nullptr;              // audio/mouth input: 7 values 0..1 (null = silence)

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
