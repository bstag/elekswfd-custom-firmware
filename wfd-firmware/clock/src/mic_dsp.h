// Microphone signal processing: 256-sample frames at 8 kHz -> two spectrum columns (bass,
// treble; 0..12) and 7 bands (0..1) for the grid. Plain C++ with no Arduino dependencies, so
// wfd-control/tests compiles it on the PC and feeds it synthetic signals.
//
// The mic's op-amp output rests near 0 V and swings upward with sound; with the ADC at 11 dB
// its ~0.1 V dead zone hides much of the room's background hum. Loudness is measured in dB above
// an adaptive noise floor: a recording of a real room (tests/host/mictrace_room.txt) spans ~40 dB
// from quiet (RMS ~15) to music and claps (RMS ~1000+), which a linear, peak-scaled meter can't
// show without either lighting up in silence or going dark after one loud sound.
#pragma once
#include <math.h>
#include <stdint.h>
#include <string.h>

namespace mic {

static const int N = 256;                       // samples per frame (32 ms)
static const int RATE = 8000;                   // Hz
static const int NBANDS = 7;
static const float EDGES[NBANDS + 1] = {60, 150, 300, 600, 1200, 2000, 3000, 4000};
static const int LEVELS = 12;                   // spectrum column height

// In-place radix-2 complex FFT, n a power of two.
inline void fft(float *re, float *im, int n) {
  for (int i = 1, j = 0; i < n; i++) {          // bit-reversal permutation
    int bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) { float t = re[i]; re[i] = re[j]; re[j] = t; t = im[i]; im[i] = im[j]; im[j] = t; }
  }
  for (int len = 2; len <= n; len <<= 1) {
    float ang = -2.0f * (float)M_PI / len, wr = cosf(ang), wi = sinf(ang);
    for (int i = 0; i < n; i += len) {
      float cr = 1, ci = 0;
      for (int k = 0; k < len / 2; k++) {
        int a = i + k, b = a + len / 2;
        float xr = re[b] * cr - im[b] * ci, xi = re[b] * ci + im[b] * cr;
        re[b] = re[a] - xr; im[b] = im[a] - xi;
        re[a] += xr; im[a] += xi;
        float t = cr * wr - ci * wi; ci = cr * wi + ci * wr; cr = t;
      }
    }
  }
}

// Loudness in dB above an adaptive noise floor, as 0..1. Tuned on mictrace_room.txt.
struct Loudness {
  float gateDb = 12;                            // must be this far above the floor to show at all
  float rangeDb = 28;                           // gate .. gate+range fills the column
  float upDb = 0.01f, downDb = 0.05f;           // floor creeps up 0.3 dB/s, falls 1.5 dB/s:
  float floor = -1;                             //   it settles near the room's quiet level
  float operator()(float rms) {
    float db = 20 * log10f(rms > 4 ? rms : 4);  // the dead zone gives frames of 0: ignore them
    if (floor < 0) floor = db;
    floor += db > floor ? upDb : -downDb;
    float f = (db - floor - gateDb) / rangeDb;
    return f <= 0 ? 0 : f > 1 ? 1 : f;
  }
};

struct Dsp {
  Loudness loud;                                // overall loudness drives everything
  float disp[2] = {0, 0};
  int colBass = 0, colTreble = 0;               // spectrum columns, 0..LEVELS
  float bands[NBANDS] = {0};                    // grid bands, 0..1
  float level = 0;                              // loudness above the floor, 0..1
  float rms = 0, lowAmp = 0, highAmp = 0;       // raw inputs of the last frame (for tracing/tuning)
  float win[N];
  float re[N], im[N];

  Dsp() {
    for (int i = 0; i < N; i++) win[i] = 0.5f - 0.5f * cosf(2 * (float)M_PI * i / (N - 1));   // Hann
  }

  // Rise at once, fall ~0.8 level per frame (as in the PC app).
  int smooth(int i, float frac) {
    float target = LEVELS * frac;
    disp[i] = target >= disp[i] ? target : (disp[i] - 0.8f > target ? disp[i] - 0.8f : target);
    int v = (int)(disp[i] + 0.5f);
    return v > LEVELS ? LEVELS : v;
  }

  void process(const uint16_t *s) {
    float mean = 0;
    for (int i = 0; i < N; i++) mean += s[i];
    mean /= N;
    float ac = 0;
    for (int i = 0; i < N; i++) {
      float d = s[i] - mean;
      ac += d * d;
      re[i] = d * win[i];
      im[i] = 0;
    }
    rms = sqrtf(ac / N);
    level = loud(rms);
    fft(re, im, N);
    float e[NBANDS] = {0};                      // band energy (sum of |bin|^2)
    for (int k = 1; k < N / 2; k++) {
      float f = (float)k * RATE / N;
      for (int b = 0; b < NBANDS; b++)
        if (f >= EDGES[b] && f < EDGES[b + 1]) { e[b] += re[k] * re[k] + im[k] * im[k]; break; }
    }
    // Shape: each band's amplitude relative to the strongest band this frame. Loudness sets
    // the height, shape spreads it, so a low tone and a high tone look different.
    float top = 0, lo = 0, hi = 0;
    for (int b = 0; b < NBANDS; b++) {
      if (e[b] > top) top = e[b];
      if (b < 3) lo += e[b]; else hi += e[b];
    }
    for (int b = 0; b < NBANDS; b++) bands[b] = top > 0 ? level * sqrtf(e[b] / top) : 0;
    lowAmp = sqrtf(lo); highAmp = sqrtf(hi);
    float big = lo > hi ? lo : hi;
    colBass = smooth(0, big > 0 ? level * sqrtf(lo / big) : 0);
    colTreble = smooth(1, big > 0 ? level * sqrtf(hi / big) : 0);
  }
};

}  // namespace mic
