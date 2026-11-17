// WiFi (station and the WFD-Setup access point), NTP, mDNS and the app's UDP display stream.
#pragma once

void startAP(bool byButton = false);  // open the "WFD-Setup" network
void wifiBegin();                     // from setup(): join the saved network, or open the setup AP
extern uint32_t radioRefreshMs;       // re-apply the transmit power this often while connected (0 = never)
extern uint32_t radioRefreshes;       // how many times that has been done
void wifiLoop();                      // every loop() pass
void netPoll();                       // the app's UDP packets
