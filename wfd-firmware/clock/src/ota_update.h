// Firmware upload over the web (POST /update) and its log (otaLog in clock_state.h).
#pragma once

extern bool otaOk;                    // set by the upload handler, read by the final POST handler
void handleUpdateUpload();
