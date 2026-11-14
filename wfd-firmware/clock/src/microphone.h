// The clock's microphone: the sampling task (core 0) and the MIC / MICLOG diagnostics.
#pragma once

void micTask(void *);
void micCapture(int n, int rate);
void micLog(int secs);
