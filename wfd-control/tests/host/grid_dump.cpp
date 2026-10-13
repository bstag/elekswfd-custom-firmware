// Host build of the firmware's grid engine, for tests/test_grid_engine.py.
//   grid_dump <anim name> <frames>   ->  one line per frame: 7 row bytes
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
  for (int n = atoi(argv[2]); n > 0; n--) {
    eng.frame(a, rows);
    printf("%d %d %d %d %d %d %d\n", rows[0], rows[1], rows[2], rows[3], rows[4], rows[5], rows[6]);
  }
  return 0;
}
