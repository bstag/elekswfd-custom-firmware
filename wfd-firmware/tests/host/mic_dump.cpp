// Host build of the firmware's mic processing, for tests/test_mic_dsp.py.
//   mic_dump <tone Hz> <tone amplitude> [frames]
// Simulates what the clock's ADC sees (11 dB: the op-amp output rests near 0 V, the ADC reads 0
// below its ~0.1 V dead zone): room hum + noise for 60 frames, the tone added for the next 40,
// then hum again. Hz 0 = no tone; amplitude 0 and Hz 0 = dead silence (all zeros).
// One line per frame: colBass colTreble level*100 band0*100 .. band6*100
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mic_dsp.h"

static uint32_t lcg = 777;
static float noise() { lcg = lcg * 1664525u + 1013904223u; return ((lcg >> 8) / 16777216.0f) * 2 - 1; }

// mic_dump replay <file>: per-frame RMS values from a MICTRACE recording ("T ms rms ..." lines)
// -> "column level%" per frame, through the same loudness + falloff the clock uses.
static int replay(const char *path) {
  FILE *f = fopen(path, "r");
  if (!f) return 4;
  static mic::Dsp dsp;
  char line[128];
  unsigned long ms;
  int rms;
  while (fgets(line, sizeof line, f))
    if (sscanf(line, "T %lu %d", &ms, &rms) == 2) {
      dsp.level = dsp.loud((float)rms);
      printf("%d %d\n", dsp.smooth(0, dsp.level), (int)(dsp.level * 100));
    }
  fclose(f);
  return 0;
}

int main(int argc, char **argv) {
  if (argc == 3 && !strcmp(argv[1], "replay")) return replay(argv[2]);
  if (argc < 3) return 2;
  float hz = atof(argv[1]), amp = atof(argv[2]);
  int frames = argc > 3 ? atoi(argv[3]) : 140;
  bool silent = hz == 0 && amp == 0;
  static mic::Dsp dsp;
  uint16_t buf[mic::N];
  long t = 0;
  for (int f = 0; f < frames; f++) {
    bool tone = hz > 0 && f >= 60 && f < 100;
    for (int i = 0; i < mic::N; i++, t++) {
      double x = t / (double)mic::RATE;
      double v = silent ? 0 : 40 + 90 * sin(2 * M_PI * 110 * x) + 30 * noise();   // hum + hiss
      if (tone) v += amp * sin(2 * M_PI * hz * x);
      v = v < 120 ? 0 : v - 120;                  // ADC dead zone eats the bottom
      buf[i] = (uint16_t)(v > 4095 ? 4095 : v);
    }
    dsp.process(buf);
    printf("%d %d %d", dsp.colBass, dsp.colTreble, (int)(dsp.level * 100));
    for (int b = 0; b < mic::NBANDS; b++) printf(" %d", (int)(dsp.bands[b] * 100));
    printf("\n");
  }
  return 0;
}
