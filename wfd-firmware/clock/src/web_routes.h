// The web server's pages and JSON API (port 80).
#pragma once
#include <WebServer.h>

extern WebServer web;
void startWebRoutes();                // register the routes and start the server (once)
