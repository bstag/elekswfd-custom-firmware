// Host build of the firmware's extras.h, for tests/test_extras.py.
//   extras_dump timer <secs>                         -> the message-line text, quoted
//   extras_dump forecast <item> <hi> <lo> <thi> <tday> <unit>
//   extras_dump time <text>                          -> "hh mm" or "invalid"
//   extras_dump due <wday> <hh> <mm>                 -> index of the due alarm in a fixed set, or -1
//   extras_dump json <key> <n> <document>            -> the numbers read, space-separated
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "extras.h"

int main(int argc, char **argv) {
  if (argc < 2) return 2;
  const char *cmd = argv[1];
  char buf[8];
  if (!strcmp(cmd, "timer") && argc == 3) {
    extras::timerText((uint32_t)strtoul(argv[2], nullptr, 10), buf);
    printf("\"%s\"\n", buf);
  } else if (!strcmp(cmd, "forecast") && argc == 8) {
    extras::forecastText(atoi(argv[2]), atoi(argv[3]), atoi(argv[4]), atoi(argv[5]), atoi(argv[6]), argv[7][0], buf);
    printf("\"%s\"\n", buf);
  } else if (!strcmp(cmd, "time") && argc == 3) {
    int hh, mm;
    if (extras::parseTime(argv[2], &hh, &mm)) printf("%d %d\n", hh, mm);
    else printf("invalid\n");
  } else if (!strcmp(cmd, "due") && argc == 5) {
    // 0: weekdays 06:30   1: off, every day 07:00   2: Sat+Sun 09:15   3: every day 23:59
    const extras::Alarm set[4] = {{1, 6, 30, 0x1F}, {0, 7, 0, 0x7F}, {1, 9, 15, 0x60}, {1, 23, 59, 0x7F}};
    printf("%d\n", extras::alarmDue(set, 4, atoi(argv[2]), atoi(argv[3]), atoi(argv[4])));
  } else if (!strcmp(cmd, "json") && argc == 5) {
    double out[8];
    int n = extras::jsonArray(argv[4], nullptr, argv[2], out, atoi(argv[3]));
    for (int i = 0; i < n; i++) printf(i ? " %g" : "%g", out[i]);
    printf("\n");
  } else {
    return 3;
  }
  return 0;
}
