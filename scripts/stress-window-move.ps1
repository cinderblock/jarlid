#Requires -Version 7
<#
.SYNOPSIS
    Regression stress test for the window-state saver deadlock fixed in v0.6.12.

.DESCRIPTION
    v0.6.11 and earlier could hang with the window still on screen, unresponsive,
    and playback stopped — no crash, nothing in the Windows event log.

    The cause was a two-thread deadlock over tauri-plugin-window-state's cache
    mutex. The debounced saver thread called `save_window_state` directly, which
    takes that mutex and holds it across `is_maximized()` / `outer_position()`.
    Off the main thread each of those blocks on a round-trip to the event loop —
    and the event loop could already be inside the plugin's own Moved/Resized
    handler, waiting for that same mutex. Neither side could proceed.

    The fix marshals the save onto the main thread with `run_on_main_thread`, so
    the getters resolve inline and the cache is only ever locked from one thread.

    This script drives the race deliberately: a burst of rapid window moves arms
    the debounce, a ~900ms pause lets the save fire, then a second burst collides
    with the in-flight save. Against v0.6.11 this reproduced the hang within ~50
    cycles; the fixed build survives many times that.

    Detection relies on SendMessageTimeout(WM_NULL): a window whose thread has
    stopped pumping messages will not answer it. Note that SetWindowPos against a
    deadlocked window blocks too, so a wedged run may stall rather than report —
    check the process with `(Get-Process jarlid).Responding` if it goes quiet.

.PARAMETER Exe
    Path to the jarlid.exe under test.

.PARAMETER Cycles
    Burst cycles to run. Default 120 (~3 min).

.EXAMPLE
    ./scripts/stress-window-move.ps1 -Exe app/src-tauri/target/release/jarlid.exe

.OUTPUTS
    Exit 0 = survived, 1 = deadlock detected, 2 = window never appeared.
#>
param(
    [Parameter(Mandatory)][string]$Exe,
    [int]$Cycles = 120
)

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class W {
    [DllImport("user32.dll")] public static extern bool SetWindowPos(
        IntPtr hWnd, IntPtr after, int X, int Y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(
        IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam, uint flags, uint timeout, out IntPtr result);
}
'@

$SWP_NOSIZE = 0x0001; $SWP_NOZORDER = 0x0004; $SWP_NOACTIVATE = 0x0010
$MOVE = $SWP_NOSIZE -bor $SWP_NOZORDER -bor $SWP_NOACTIVATE

function Test-Pumping([IntPtr]$hwnd) {
    $res = [IntPtr]::Zero
    # WM_NULL, SMTO_ABORTIFHUNG, 3s. Zero return => not pumping messages.
    $r = [W]::SendMessageTimeout($hwnd, 0, [IntPtr]::Zero, [IntPtr]::Zero, 0x2, 3000, [ref]$res)
    return $r -ne [IntPtr]::Zero
}

Write-Host "launching $Exe"
$proc = Start-Process -FilePath $Exe -PassThru
$hwnd = [IntPtr]::Zero
foreach ($i in 1..60) {
    Start-Sleep -Milliseconds 500
    $proc.Refresh()
    if ($proc.MainWindowHandle -ne [IntPtr]::Zero) { $hwnd = $proc.MainWindowHandle; break }
}
if ($hwnd -eq [IntPtr]::Zero) {
    Write-Host "no main window appeared - aborting"
    $proc.Kill(); exit 2
}
Write-Host "pid=$($proc.Id) hwnd=$hwnd - starting $Cycles cycles"

foreach ($c in 1..$Cycles) {
    # Burst 1 - arms the 800ms debounce.
    foreach ($j in 1..25) {
        [void][W]::SetWindowPos($hwnd, [IntPtr]::Zero, 120 + (($j * 13) % 260), 120 + (($j * 7) % 160), 0, 0, $MOVE)
        Start-Sleep -Milliseconds 8
    }
    # Let the saver arm and fire (fires 800-1300ms after the last move).
    Start-Sleep -Milliseconds 900
    # Burst 2 - collide with the in-flight save.
    foreach ($j in 1..40) {
        [void][W]::SetWindowPos($hwnd, [IntPtr]::Zero, 140 + (($j * 11) % 240), 140 + (($j * 17) % 150), 0, 0, $MOVE)
        Start-Sleep -Milliseconds 6
    }

    if (-not (Test-Pumping $hwnd)) {
        Write-Host "*** DEADLOCK at cycle $c - window stopped pumping messages ***"
        Write-Host "pid=$($proc.Id) left running; dump it with: procdump -ma $($proc.Id) hang.dmp"
        exit 1
    }
    if ($c % 20 -eq 0) { Write-Host "  cycle $c ok" }
}

Write-Host "SURVIVED all $Cycles cycles - still responsive"
$proc.Kill()
exit 0
