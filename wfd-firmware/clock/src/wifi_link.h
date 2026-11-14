// WiFi (station and the WFD-Setup access point), NTP, mDNS and the app's UDP display stream.
#pragma once

void startAP(bool byButton = false);  // open the "WFD-Setup" network
void wifiBegin();                     // from setup(): join the saved network, or open the setup AP
void wifiLoop();                      // every loop() pass
void netPoll();                       // the app's UDP packets
