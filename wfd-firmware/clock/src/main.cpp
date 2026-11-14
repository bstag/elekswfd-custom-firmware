// EleksWFD clock firmware (our own). Shows DS3231 time on the panel; time is set over USB.
// Serial @115200:
//   SET yy mo dd dow hh mi ss   set RTC (yy=2-digit year, dow 1=Mon..7=Sun), clears the lost-power flag
//   GET                         prints "yy mo dd dow hh mi ss osf"
// The rest lives in the other files here (see docs/FIRMWARE.md, "Source layout"); this one is
// setup(), loop() and the build stamp.
#include <Arduino.h>
#include <Wire.h>
#include "clock_state.h"
#include "grid_engine.h"
#include "buttons_alerts.h"
#include "clock_display.h"
#include "clock_settings.h"
#include "microphone.h"
#include "serial_cmds.h"
#include "standalone.h"
#include "wifi_link.h"

// What the PC app checks before talking to this firmware. Bump WFD_PROTOCOL whenever commands,
// settings or the JSON API change in a way the app must know about (PROTOCOL in wfd-app/src/config.rs).
//   1 serial display commands   2 settings over USB (CFG/CFGSET) + JSON API   3 WiFi link (UDP, BYE, /api/frames)
//   4 "weather" grid animation + weather_scene setting
//   5 alarms, timer, alert settings + forecast (message source, state.weather.forecast)
#define WFD_PROTOCOL 5
const char *FW_BUILD = "wfd-clock " __DATE__ " " __TIME__;
const int fwProtocol = WFD_PROTOCOL;

static const int SDA_PIN = 21, SCL_PIN = 22;
static grid::Engine gEng(esp_random);
static uint32_t gaStep = 0;
static String line;

void setup() {
  Serial.begin(115200);
  pinMode(25, INPUT);
  pinMode(26, INPUT);
  pinMode(27, INPUT);
  Wire.begin(SDA_PIN, SCL_PIN, 100000);
  delay(100);
  tmInit(6);
  prefs.begin("wfd", false);
  specStyle = prefs.getInt("style", 4);   // restore saved style, default = peak dot
  brightness = prefs.getInt("bright", 6);
  clock24 = prefs.getInt("clk24", 1);
  tmCmd(0xB0 | ((brightness - 1) & 0x0F));
  gaN = constrain(prefs.getInt("gn", 0), 0, GA_MAX);   // restore stored standalone animation
  gaFps = prefs.getInt("gfps", 8);
  gaFlag = prefs.getInt("gflag", 0);
  gaAuto = prefs.getInt("gauto", 0);
  if (gaN > 0 && prefs.getBytes("ganim", gaFrames, gaN * 7) != (size_t)gaN * 7) gaN = 0;
  // stored frames keep playing after the upgrade; otherwise default to a built-in animation
  gaAnim = constrain(prefs.getInt("gaid", gaN > 0 ? grid::CUSTOM : grid::SPIN), 0, grid::N_ANIM - 1);
  webPass = prefs.getString("webpass", "");
  wxScenePick = constrain(prefs.getInt("wxsc", -1), -1, grid::N_SCENE - 1);
  if (prefs.getBytes("alarms", alarms, sizeof alarms) != sizeof alarms) memset(alarms, 0, sizeof alarms);
  otaLogValid = prefs.getBytes("otalog", &otaLog, sizeof otaLog) == sizeof otaLog;   // last upload
  micAuto = prefs.getInt("micauto", 1);
  analogSetPinAttenuation(MIC_PIN, ADC_11db);   // dead zone hides the room's hum (see mic_dsp.h)
  gEng.bands = micBands;
  xTaskCreatePinnedToCore(micTask, "mic", 4096, nullptr, 1, nullptr, 0);
  omAuto = prefs.getInt("omauto", 0);    // restore the message-line engine
  msgMode = constrain(prefs.getInt("mmode", SRC_TEXT), 0, N_SRC - 1);
  msgRot = prefs.getInt("mrot", 0b111);
  msgDwell = prefs.getInt("mdwell", 8);
  msgScroll = prefs.getInt("mscroll", 350);
  msgText = prefs.getString("omtext", "WFD CLOCK");
  prefs.remove("om"); prefs.remove("omn"); prefs.remove("omfps");   // pre-rendered frames: no longer used
  wifiBegin();
  Serial.printf("wfd-clock ready, style %d, bright %d, grid %d auto %d, msg mode %d auto %d, wifi %s\n",
                specStyle, brightness, gaN, gaAuto, msgMode, omAuto, apMode ? "setup-AP" : "connecting");
}

void loop() {
  static uint32_t last = 0;
  while (Serial.available()) {
    char c = Serial.read();
    if (c == '\n' || c == '\r') { if (line.length()) { handle(line); line = ""; } }
    else if (line.length() < 1024) line += c;   // room for CFGSET {json}
  }
  wifiLoop();
  if (netUp) netPoll();
  checkButtons();
  // advance stored animations only when the app is off
  bool redraw = false;
  bool appOff = !appTime.within(APP_ON_MS);
  if (appOff && gaAuto && gridReady() &&
      millis() - gaStep > (uint32_t)(1000 / (gaFps < 1 ? 1 : gaFps))) {
    gaStep = millis();
    if (gaAnim == grid::CUSTOM) gaIdx = (gaIdx + 1) % gaN;
    else { gEng.scene = currentScene(); gEng.frame(gaAnim, gEngRows); }
    redraw = true;
  }
  if (appOff && omAuto && (msgStepAt == 0 || millis() - msgStepAt >= (uint32_t)msgScroll)) {
    msgStepAt = millis();
    msgStep();
    redraw = true;
  }
  if (millis() - last >= 250) {
    last = millis();
    if (rtcRead(curT)) redraw = true;
  }
  static uint32_t extrasAt = 0;
  if (millis() - extrasAt >= 200) {
    extrasAt = millis();
    if (extrasTick()) redraw = true;
  }
  if (infoText.length()) {             // the notice: scroll it, drop it when its time is up
    if (!infoOn()) { infoText = ""; redraw = true; }
    else if (infoText.length() > 6 && millis() - infoStepAt >= 350) { infoStepAt = millis(); infoPos++; redraw = true; }
  }
  if (redraw || dirty) {               // one I2C flush per pass, however many commands arrived
    dirty = false;
    draw();
  }
}
