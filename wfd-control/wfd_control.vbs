' Launch WFD Control hidden at login. Serves the UI at http://127.0.0.1:8737
Set sh = CreateObject("WScript.Shell")
sh.CurrentDirectory = "E:\Source\elekstube\EleksWFD_Tools_v3_1_EN_1104\wfd-control"
sh.Run """E:\Source\elekstube\EleksWFD_Tools_v3_1_EN_1104\wfd-firmware\.venv\Scripts\pythonw.exe"" ""E:\Source\elekstube\EleksWFD_Tools_v3_1_EN_1104\wfd-control\app.py"" --no-browser", 0, False
