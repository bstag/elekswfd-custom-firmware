' Launch WFD Control hidden at login. Serves the UI at http://127.0.0.1:8737
' Uses the venv in ..\wfd-firmware\.venv (see docs/APP.md); paths are relative to this file.
Set fso = CreateObject("Scripting.FileSystemObject")
Set sh = CreateObject("WScript.Shell")
here = fso.GetParentFolderName(WScript.ScriptFullName)
pyw = fso.BuildPath(fso.GetParentFolderName(here), "wfd-firmware\.venv\Scripts\pythonw.exe")
sh.CurrentDirectory = here
sh.Run """" & pyw & """ """ & fso.BuildPath(here, "app.py") & """ --no-browser", 0, False
