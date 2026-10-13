' Launch the WFD background helper hidden at login. Delete this file (or its Startup copy) to disable.
Set sh = CreateObject("WScript.Shell")
sh.CurrentDirectory = "E:\Source\elekstube\EleksWFD_Tools_v3_1_EN_1104\wfd-firmware"
sh.Run """E:\Source\elekstube\EleksWFD_Tools_v3_1_EN_1104\wfd-firmware\.venv\Scripts\pythonw.exe"" ""E:\Source\elekstube\EleksWFD_Tools_v3_1_EN_1104\wfd-firmware\wfd_daemon.py""", 0, False
