// When the app is off: the grid animation settings and the on-device message line engine.
#include <Arduino.h>
#include "clock_state.h"
#include "clock_display.h"
#include "standalone.h"

// Choose what the grid plays when the app is off (turns that on), and persist it.
void setGridAnim(int anim, int fps, int flag) {
  gaAnim = constrain(anim, 0, grid::N_ANIM - 1);
  gaFps = constrain(fps, 1, 30);
  gaFlag = flag ? 1 : 0;
  gaAuto = 1;
  gaIdx = 0;
  prefs.putInt("gaid", gaAnim); prefs.putInt("gfps", gaFps);
  prefs.putInt("gflag", gaFlag); prefs.putInt("gauto", gaAuto);
}

// Store drawn grid frames (rows: n x 7 bytes) and play them when the app is off.
void storeFrames(const uint8_t *rows, int n, int fps, int flag) {
  gaN = constrain(n, 0, GA_MAX);
  gaIdx = 0;
  for (int i = 0; i < gaN * 7; i++) gaFrames[i] = rows[i] & 0x7F;
  prefs.putBytes("ganim", gaFrames, gaN * 7);
  prefs.putInt("gn", gaN);
  setGridAnim(grid::CUSTOM, fps, flag);
}

// ---- on-device message engine ----
static const char *DAY_ABBR[7] = {"MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"};
static const char *DAY_FULL[7] = {"MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY", "SATURDAY", "SUNDAY"};
static const char *MON_ABBR[12] = {"JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"};

static String tempText() { return String(wxTemp) + (wxUnit ? "C" : "F"); }

// Expand {time} {time12} {date} {day} {temp} from the RTC / weather (same as the PC app).
static String expandMsg(String s) {
  if (s.indexOf('{') < 0) return s;
  const uint8_t *t = curT;
  int hh = bcd2dec(t[2] & 0x3F), mi = bcd2dec(t[1]);
  int dow = constrain((t[3] & 7) - 1, 0, 6), mo = constrain(bcd2dec(t[5] & 0x1F) - 1, 0, 11);
  char buf[16];
  snprintf(buf, sizeof buf, "%02d:%02d", hh, mi);
  s.replace("{time}", buf);
  snprintf(buf, sizeof buf, "%d:%02d%s", hh % 12 ? hh % 12 : 12, mi, hh < 12 ? "AM" : "PM");
  s.replace("{time12}", buf);
  snprintf(buf, sizeof buf, "%s %s %02d", DAY_ABBR[dow], MON_ABBR[mo], bcd2dec(t[4]));
  s.replace("{date}", buf);
  s.replace("{day}", DAY_FULL[dow]);
  s.replace("{temp}", wxFresh() ? tempText() : String("--"));
  return s;
}

static int rotSrc = N_SRC - 1, scrollPos = 0;
static uint32_t rotAt = 0;
uint32_t msgStepAt = 0;

static bool srcAvailable(int src) {
  return src == MODE_ROTATE ? false : src == SRC_TEMP ? wxFresh() : src == SRC_FORECAST ? fcFresh() : true;
}
int todayIdx() { return constrain((curT[3] & 7) - 1, 0, 6); }   // 0 = Monday

// One engine step: choose the source, render the next scroll position into engRam.
void msgStep() {
  int src = msgMode;
  if (msgMode == MODE_ROTATE) {
    bool cur = (msgRot & (1 << rotSrc)) && srcAvailable(rotSrc);
    if (!cur || millis() - rotAt >= (uint32_t)msgDwell * 1000) {
      for (int k = 1; k <= N_SRC; k++) {               // next enabled + available source
        int c = (rotSrc + k) % N_SRC;
        if ((msgRot & (1 << c)) && srcAvailable(c)) {
          if (c != rotSrc || !cur) { rotSrc = c; scrollPos = 0; msgShown = ""; }
          break;
        }
      }
      rotAt = millis();
    }
    src = (msgRot & (1 << rotSrc)) && srcAvailable(rotSrc) ? rotSrc : SRC_TEMP;
  }
  if (src == SRC_FORECAST && !fcFresh()) src = SRC_TEMP;      // no forecast yet: the temperature
  if (src == SRC_TEMP) { engTemp = true; msgShown = wxFresh() ? tempText() : ""; return; }
  engTemp = false;
  if (src == SRC_FORECAST) {                                  // HI / LO / tomorrow, 3 s each
    char buf[8];
    extras::forecastText((millis() / 3000) % 3, fcHi[0], fcLo[0], fcHi[1], todayIdx() + 1, wxUnit ? 'C' : 'F', buf);
    msgShown = buf;
    render6(msgShown, 0, engRam);
    return;
  }
  String txt = expandMsg(src == SRC_DATE ? String("{date}") : msgText);
  txt.trim();
  if (txt.length() == 0) txt = " ";
  if (txt != msgShown) { msgShown = txt; scrollPos = 0; }
  if (txt.length() <= 6) {
    render6(txt, 0, engRam);
  } else {                                             // wrap with a 3-space gap, like the app
    String d = txt + "   ";
    render6(d + d, scrollPos % d.length(), engRam);
    scrollPos++;
  }
}

void setMsgText(String t) {
  t.replace("\t", " ");
  t.trim();
  if (t.length() > MSG_TEXT_MAX) t = t.substring(0, MSG_TEXT_MAX);
  if (t == msgText) return;
  msgText = t;
  prefs.putString("omtext", msgText);
  msgStepAt = 0;                                       // show it now
}

void setMsgCfg(int mode, int rot, int dwell, int scroll) {
  mode = constrain(mode, 0, N_SRC - 1);
  rot &= ((1 << N_SRC) - 1) & ~(1 << MODE_ROTATE);
  dwell = constrain(dwell, 2, 600);
  scroll = constrain(scroll, 50, 5000);
  if (mode == msgMode && rot == msgRot && dwell == msgDwell && scroll == msgScroll) return;
  msgMode = mode; msgRot = rot; msgDwell = dwell; msgScroll = scroll;
  prefs.putInt("mmode", msgMode); prefs.putInt("mrot", msgRot);
  prefs.putInt("mdwell", msgDwell); prefs.putInt("mscroll", msgScroll);
  rotAt = 0; msgStepAt = 0;
}
