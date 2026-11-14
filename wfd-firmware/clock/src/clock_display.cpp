// The panel and the RTC over I2C, and draw(): rebuilds the TM1680 RAM from the shared state.
#include <Arduino.h>
#include <Wire.h>
#include "clock_state.h"
#include "buttons_alerts.h"
#include "microphone.h"
#include "nvs_store.h"
#include "weather_fetch.h"
#include "clock_panel.h"
#include "font14_table.h"
#include "clock_display.h"

static const uint8_t TM = 0x73, RTC = 0x68;
static uint8_t ram[WFD_RAM_BYTES];

void tmCmd(uint8_t c) { Wire.beginTransmission(TM); Wire.write(c); Wire.endTransmission(); }
void tmInit(uint8_t pwm) {
  const uint8_t s[] = {0x80, 0xA4, 0x98, 0x81, 0x83, (uint8_t)(0xB0 | (pwm & 0xF)), 0x88};
  for (uint8_t c : s) tmCmd(c);
}
static void tmFlush() {
  Wire.beginTransmission(TM);
  Wire.write(0x00);
  for (int i = 0; i < WFD_RAM_BYTES; i++) Wire.write(ram[i]);
  Wire.endTransmission();
}

uint8_t bcd2dec(uint8_t v) { return (v >> 4) * 10 + (v & 0x0F); }
static uint8_t dec2bcd(uint8_t v) { return ((v / 10) << 4) | (v % 10); }

bool rtcRead(uint8_t *o) {
  Wire.beginTransmission(RTC); Wire.write(0);
  if (Wire.endTransmission() != 0) return false;
  if (Wire.requestFrom(RTC, (uint8_t)7) != 7) return false;
  for (int i = 0; i < 7; i++) o[i] = Wire.read();
  return true;
}
uint8_t rtcStatus() {
  Wire.beginTransmission(RTC); Wire.write(0x0F); Wire.endTransmission();
  Wire.requestFrom(RTC, (uint8_t)1); return Wire.read();
}
void rtcSet(int yy, int mo, int dd, int dow, int hh, int mi, int ss) {
  Wire.beginTransmission(RTC); Wire.write(0);
  Wire.write(dec2bcd(ss)); Wire.write(dec2bcd(mi)); Wire.write(dec2bcd(hh));
  Wire.write(dow & 0x07); Wire.write(dec2bcd(dd)); Wire.write(dec2bcd(mo)); Wire.write(dec2bcd(yy));
  Wire.endTransmission();
  uint8_t st = rtcStatus();                       // clear OSF (lost-power flag)
  Wire.beginTransmission(RTC); Wire.write(0x0F); Wire.write(st & 0x7F); Wire.endTransmission();
}

struct Day { uint8_t byte, mask; };
static const Day DAYS[7] = {
  {DAY_MON_BYTE, DAY_MON_MASK}, {DAY_TUE_BYTE, DAY_TUE_MASK}, {DAY_WED_BYTE, DAY_WED_MASK},
  {DAY_THU_BYTE, DAY_THU_MASK}, {DAY_FRI_BYTE, DAY_FRI_MASK}, {DAY_SAT_BYTE, DAY_SAT_MASK},
  {DAY_SUN_BYTE, DAY_SUN_MASK},
};

static void drawGrid(const uint8_t *rows, bool frame) {   // rows[r] bit c = cell (r,c)
  for (int r = 0; r < 7; r++)
    for (int c = 0; c < 7; c++)
      if (rows[r] & (1 << c)) {
        int i = r * 7 + c;
        ram[GRID_BYTE[i]] |= GRID_MASK[i];
      }
  if (frame)
    for (int i = 0; i < GRID_FRAME_N; i++) ram[GRID_FRAME_BYTE[i]] |= GRID_FRAME_MASK[i];
}

static void drawWeatherWord() {
  const uint8_t *w = wx.code == 1 ? WORD_SUNNY : wx.code == 2 ? WORD_CLOUD : wx.code == 3 ? WORD_RAINS : nullptr;
  if (w) { ram[w[0]] |= w[1]; if (w[2] || w[3]) ram[w[2]] |= w[3]; }
}

static void drawTemp() {
  int t = wx.temp, neg = t < 0; if (neg) t = -t;
  ram[ALNUM_LO[5]] = wx.unit ? CH_C : CH_F;         // rightmost char: F or C
  ram[ALNUM_LO[4]] = DIGIT_FONT[t % 10];           // units
  if (t >= 10) ram[ALNUM_LO[3]] = DIGIT_FONT[(t / 10) % 10];   // tens
  if (t >= 100) ram[ALNUM_LO[2]] = DIGIT_FONT[(t / 100) % 10]; // hundreds
  if (neg) ram[ALNUM_LO[t >= 10 ? 2 : 3]] = 0x04;  // minus sign = middle segment
}

static void drawBar(const uint8_t *BY, const uint8_t *MK, int pct,
                    uint8_t lb, uint8_t lm, uint8_t pb, uint8_t pm) {
  ram[lb] |= lm;                       // label (CPU/GPU/RAM text)
  ram[pb] |= pm;                       // percent sign
  int segs = (pct * 20 + 50) / 100;
  if (segs < 0) segs = 0; if (segs > 20) segs = 20;
  for (int i = 0; i < segs; i++) ram[BY[i]] |= MK[i];
}

void setStyle(int v) {
  if (v < 1 || v > N_STYLES) return;
  if (v != disp.style) { disp.style = v; prefSave(P_STYLE); }  // remember across power-off
}

void setBrightness(int v) {
  if (v < 1) v = 1; if (v > 16) v = 16;
  disp.brightness = v;
  tmCmd(0xB0 | ((v - 1) & 0x0F));
  prefSave(P_BRIGHT);
}

static int peakVal[2] = {0, 0};
static uint32_t peakAt[2] = {0, 0};

static void drawCol(const uint8_t *BY, const uint8_t *MK, int level, int col) {
  if (level < 0) level = 0; if (level > 12) level = 12;
  int lo = 6 - level / 2, hi = 6 + (level + 1) / 2;   // for center style
  // peak-hold: rise instantly, fall ~1 level per 90 ms
  if (level >= peakVal[col]) { peakVal[col] = level; peakAt[col] = millis(); }
  else if (millis() - peakAt[col] > 90) { peakVal[col]--; peakAt[col] = millis(); }
  for (int i = 0; i < 12; i++) {                       // i=0 bottom .. i=11 top
    bool on = false;
    switch (disp.style) {
      case 1: on = i < level; break;                   // bars from bottom
      case 2: on = i >= 12 - level; break;             // bars from top
      case 3: on = (i >= lo && i < hi); break;         // grow from center
      case 4: on = (peakVal[col] > 0 && i == peakVal[col] - 1); break; // floating peak dot
      case 5: on = (i < level) || (peakVal[col] > 0 && i == peakVal[col] - 1); break; // bars + peak dot
    }
    if (on) ram[BY[i]] |= MK[i];
  }
}

// ---- on-device text rendering (14-seg font) ----
static void glyph14(char c, uint8_t *lo, uint8_t *hi) {
  if (c >= 'a' && c <= 'z') c -= 32;
  if (c >= 32 && c < 128) { *lo = FONT14_LO[c - 32]; *hi = FONT14_HI[c - 32]; }
  else { *lo = 0; *hi = 0; }
}
void render6(const String &s, int start, uint8_t *out) {   // out[12]
  for (int i = 0; i < 6; i++) {
    int idx = start + i;
    char c = (idx >= 0 && idx < (int)s.length()) ? s[idx] : ' ';
    glyph14(c, &out[i * 2], &out[i * 2 + 1]);
  }
}

void draw() {
  memset(ram, 0, sizeof ram);
  const Standalone &sa = standalone;
  const uint8_t *t = disp.now;
  int hh = bcd2dec(t[2] & 0x3F), mi = bcd2dec(t[1]), ss = bcd2dec(t[0] & 0x7F);
  bool blankTens = false;
  if (!disp.clock24) {                  // 12-hour display
    hh %= 12; if (hh == 0) hh = 12;
    blankTens = (hh < 10);              // drop the leading zero (e.g. 1:23 not 01:23)
  }
  int d6[6] = {hh / 10, hh % 10, mi / 10, mi % 10, ss / 10, ss % 10};
  for (int i = 0; i < 6; i++)
    ram[DIGIT_BYTE[i]] = (i == 0 && blankTens) ? 0 : DIGIT_FONT[d6[i] % 10];
  for (int i = 0; i < 2; i++) ram[COLON_BYTE[i]] |= COLON_MASK[i];
  int dow = t[3] & 0x07;
  if (dow >= 1 && dow <= 7) ram[DAYS[dow - 1].byte] |= DAYS[dow - 1].mask;
  bool specLive = live.specTime.within(600);        // PC audio (clears if the PC went quiet)
  bool appOn = live.appTime.within(APP_ON_MS);
  bool micLive = !specLive && sa.micAuto && !appOn && millis() - micState.frameAt < 500;
  drawCol(SPEC_C1_BYTE, SPEC_C1_MASK, specLive ? live.specA : micLive ? micState.bass : 0, 0);      // mic: bass
  drawCol(SPEC_C2_BYTE, SPEC_C2_MASK, specLive ? live.specB : micLive ? micState.treble : 0, 1);    // mic: treble
  if (live.barsTime.within(3000)) {                 // bars when the PC is sending them
    drawBar(BAR_CPU_BYTE, BAR_CPU_MASK, live.barCpu, LBL_CPU_BYTE, LBL_CPU_MASK, PCT_CPU_BYTE, PCT_CPU_MASK);
    drawBar(BAR_GPU_BYTE, BAR_GPU_MASK, live.barGpu, LBL_GPU_BYTE, LBL_GPU_MASK, PCT_GPU_BYTE, PCT_GPU_MASK);
    drawBar(BAR_RAM_BYTE, BAR_RAM_MASK, live.barRam, LBL_RAM_BYTE, LBL_RAM_MASK, PCT_RAM_BYTE, PCT_RAM_MASK);
  }
  bool wxLive = wxFresh();
  if (wxLive) drawWeatherWord();
  if (live.msgTime.within(1500)) {                  // live scrolling text from the app
    for (int i = 0; i < 12; i++) ram[i] = live.msgRam[i];
  } else if (!appOn && sa.omAuto && !sa.engTemp) {  // app off -> on-device message engine
    for (int i = 0; i < 12; i++) ram[i] = sa.engRam[i];
  } else if (wxLive) {
    drawTemp();                                     // live temperature
  }
  if (live.gridTime.within(1500)) drawGrid(live.gridRows, live.gridFrame);    // live grid from the app
  else if (!appOn && sa.gaAuto && gridReady())                               // app off -> own animation
    drawGrid(sa.gaAnim == grid::CUSTOM ? &sa.gaFrames[sa.gaIdx * 7] : sa.gEngRows, sa.gaFlag);
  if (live.iconTime.within(1500))                    // top-row status icons
    for (int i = 0; i < N_ICONS; i++)
      if (live.iconMask & (1UL << i)) {
        ram[ICON_B0[i]] |= ICON_M0[i];
        if (ICON_M1[i]) ram[ICON_B1[i]] |= ICON_M1[i];
      }
  if (live.logoOn && live.logoTime.within(1500))     // ELEKSMAKER logo
    for (int i = 0; i < N_LOGO; i++) ram[LOGO_BYTE[i]] |= LOGO_MASK[i];
  if (alerts.cdEnd) {                             // countdown on the message line (bytes 0..11)
    char buf[8];
    extras::timerText(timerLeft(), buf);
    render6(String(buf), 0, ram);
  }
  if (infoOn()) {                                 // notice from a held button, over the message line
    if (disp.infoText.length() <= 6) render6(disp.infoText, 0, ram);
    else { String d = disp.infoText + "   "; render6(d + d, disp.infoPos % d.length(), ram); }
  }
  if (alerts.kind) {                               // flash: everything on, then the reason
    if ((millis() / 400) % 2) memset(ram, 0xFF, sizeof ram);
    else render6(String(alerts.kind == ALERT_TIMER ? " TIMER" : " ALARM"), 0, ram);
  }
  tmFlush();
}

void showInfo(const String &text, uint32_t ms) {
  disp.infoText = text;
  disp.infoUntil = millis() + ms;
  disp.infoStepAt = millis();
  disp.infoPos = 0;
  disp.dirty = true;
}
