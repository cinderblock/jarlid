# Focus the Jarlid main window and press Space (play/pause toggle).
Add-Type -AssemblyName Microsoft.VisualBasic
Add-Type -AssemblyName System.Windows.Forms

$proc = Get-Process jarlid, app -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowTitle -in @("Jarlid", "Pandora") } | Select-Object -First 1
if (-not $proc) { throw "Jarlid main window not found" }
[Microsoft.VisualBasic.Interaction]::AppActivate($proc.Id)
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait(" ")
Write-Output "space sent to PID $($proc.Id)"
