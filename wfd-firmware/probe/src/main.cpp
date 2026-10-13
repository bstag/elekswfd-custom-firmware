// WFD bridge - our own test firmware for the EleksWFD board.
// Turns the clock into a USB-to-I2C bridge so the PC can drive the TM1680 display (0x73)
// and read/set the DS3231 RTC (0x68) interactively. Line protocol at 115200 baud:
//   scan                      list I2C devices
//   w <addr> <b0> <b1> ...    I2C write (hex bytes)
//   r <addr> <reg> <n>        write register pointer, then read n bytes (hex)
//   pins <sda> <scl>          re-init I2C on other pins
//   io                        buttons + mic peak since last call
// Replies are one line: "ok ...", or "err ...".
#include <Arduino.h>
#include <Wire.h>

static int sdaPin = 21, sclPin = 22;
static int micPeak = 0;

static void startBus() {
  Wire.end();
  Wire.begin(sdaPin, sclPin, 100000);
}

static int parseHex(const String &tok) { return (int)strtol(tok.c_str(), nullptr, 16); }

static void split(const String &line, String *tok, int &n, int maxTok) {
  n = 0;
  int i = 0;
  while (i < (int)line.length() && n < maxTok) {
    while (i < (int)line.length() && line[i] == ' ') i++;
    int j = i;
    while (j < (int)line.length() && line[j] != ' ') j++;
    if (j > i) tok[n++] = line.substring(i, j);
    i = j;
  }
}

static void handle(const String &line) {
  String t[140];
  int n;
  split(line, t, n, 140);
  if (n == 0) return;
  if (t[0] == "scan") {
    String out = "ok";
    for (uint8_t a = 0x08; a < 0x78; a++) {
      Wire.beginTransmission(a);
      if (Wire.endTransmission() == 0) { char b[6]; snprintf(b, sizeof b, " %02x", a); out += b; }
    }
    Serial.println(out);
  } else if (t[0] == "w" && n >= 2) {
    Wire.beginTransmission((uint8_t)parseHex(t[1]));
    for (int i = 2; i < n; i++) Wire.write((uint8_t)parseHex(t[i]));
    int e = Wire.endTransmission();
    Serial.println(e == 0 ? String("ok") : String("err i2c ") + e);
  } else if (t[0] == "r" && n == 4) {
    uint8_t a = parseHex(t[1]);
    Wire.beginTransmission(a);
    Wire.write((uint8_t)parseHex(t[2]));
    int e = Wire.endTransmission();
    if (e) { Serial.println(String("err i2c ") + e); return; }
    int cnt = parseHex(t[3]);
    int got = Wire.requestFrom((int)a, cnt);
    String out = "ok";
    for (int i = 0; i < got; i++) { char b[6]; snprintf(b, sizeof b, " %02x", Wire.read()); out += b; }
    Serial.println(out);
  } else if (t[0] == "pins" && n == 3) {
    sdaPin = t[1].toInt();
    sclPin = t[2].toInt();
    startBus();
    Serial.printf("ok sda=%d scl=%d\n", sdaPin, sclPin);
  } else if (t[0] == "io") {
    Serial.printf("ok %d %d %d %d\n", digitalRead(25), digitalRead(26), digitalRead(27), micPeak);
    micPeak = 0;
  } else {
    Serial.println("err unknown");
  }
}

void setup() {
  Serial.begin(115200);
  pinMode(25, INPUT);
  pinMode(26, INPUT);
  pinMode(27, INPUT);
  startBus();
  delay(200);
  Serial.println("ready wfd-bridge v2");
}

void loop() {
  static String line;
  while (Serial.available()) {
    char c = Serial.read();
    if (c == '\n' || c == '\r') {
      if (line.length()) handle(line);
      line = "";
    } else if (line.length() < 1024) {
      line += c;
    }
  }
  int m = analogRead(32);
  if (m > micPeak) micPeak = m;
}
