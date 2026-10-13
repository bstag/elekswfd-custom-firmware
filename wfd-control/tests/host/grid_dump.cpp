// Host build of the firmware's grid engine, for tests/test_grid_engine.py.
//   grid_dump <anim name> <frames>   ->  one line per frame: 7 row bytes
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#include "grid_engine.h"

static uint32_t lcg = 12345;
static uint32_t testRand() { return lcg = lcg * 1664525u + 1013904223u; }

int main(int argc, char **argv) {
  if (argc != 3) return 2;
  int a = grid::animFromName(argv[1]);
  if (a <= grid::CUSTOM) return 3;
  grid::Engine eng(testRand);
  uint8_t rows[7];
  double bands[7];
  eng.bands = bands;
  for (int i = 0, n = atoi(argv[2]); i < n; i++) {
    for (int c = 0; c < 7; c++)                 // same synthetic input as test_grid_engine.py
      bands[c] = (sin(i * 0.31 + c * 0.9) + 1) / 2 * (i % 23 < 4 ? 0.0 : (i % 23) / 22.0);   // ramps, silent gaps
    eng.frame(a, rows);
    printf("%d %d %d %d %d %d %d\n", rows[0], rows[1], rows[2], rows[3], rows[4], rows[5], rows[6]);
  }
  return 0;
}
