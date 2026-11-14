// Definitions of the shared state declared in clock_state.h (initial values as before the split).
#include "clock_state.h"
#include "grid_engine.h"
#include "mic_dsp.h"

Preferences prefs;

WebServer web(80);
bool apMode = false;
bool ntpSynced = false;
String g_tz;
String g_lat, g_lon;
int g_wunit = 0;
uint32_t lastWxFetch = 0;

uint8_t curT[7] = {0};
int specA = 0, specB = 0;
Stamp specTime;
int barCpu = 0, barGpu = 0, barRam = 0;
Stamp barsTime;
int wxCode = 0, wxTemp = 0, wxUnit = 0;
Stamp wxTime;
int wxScene = -1;
uint32_t wxSceneTime = 0;
int wxScenePick = -1;
int fcN = 0, fcHi[4], fcLo[4], fcCode[4];
uint32_t fcTime = 0;
extras::Alarm alarms[extras::N_ALARMS] = {};
uint32_t cdEnd = 0;
int alertKind = ALERT_NONE;

String infoText;
uint32_t infoUntil = 0, infoStepAt = 0;
int infoPos = 0;

uint8_t msgRam[12] = {0};
Stamp msgTime;

int omAuto = 0;
int msgMode = SRC_TEXT;
int msgRot = 0b111;
int msgDwell = 8;
int msgScroll = 350;
String msgText = "WFD CLOCK";
uint8_t engRam[12] = {0};
bool engTemp = true;
String msgShown;
uint8_t gridRows[7] = {0};
uint8_t gridFrame = 0;
Stamp gridTime;
Stamp appTime;
uint32_t iconMask = 0;
Stamp iconTime;
int logoOn = 0;
Stamp logoTime;
bool dirty = false;

int specStyle = 4;
int clock24 = 1;
int brightness = 6;

uint8_t gaFrames[GA_MAX * 7];
int gaN = 0, gaFps = 8, gaFlag = 0, gaAuto = 0;
int gaIdx = 0;
int gaAnim = grid::SPIN;
uint8_t gEngRows[7] = {0};

int micAuto = 1;
volatile int micBass = 0, micTreble = 0, micLevel = 0;
double micBands[mic::NBANDS] = {0};
volatile uint32_t micFrameAt = 0;
volatile bool micPause = false;
volatile uint32_t micTraceUntil = 0;

bool netUp = false;
uint32_t udpPackets = 0, udpRejected = 0;
String webPass;

OtaLog otaLog = {};
bool otaLogValid = false;
