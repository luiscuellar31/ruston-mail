# ==============================================================================
# Ruston Mail - Windows startup smoke test
# Starts ruston.exe and fails unless its window opens and draws without an
# error box, then closes it as the title bar's close button does and waits for
# the process to end. A failure to start shows a message box rather than
# ending the process, so a process still running proves nothing on its own.
# ==============================================================================
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,

    # How long the window may take to appear.
    [int]$Seconds = 30
)

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class RustonSmokeWindows
{
    delegate bool EnumProc(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr parameter);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr window, StringBuilder name, int size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr window, StringBuilder text, int size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern uint GetDlgItemTextW(IntPtr dialog, int item, StringBuilder text, int size);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr window, uint message, IntPtr w, IntPtr l);

    /// Visible top-level windows of a process, as "class|title" with the handle.
    public static Dictionary<IntPtr, string> VisibleWindowsOf(uint processId)
    {
        var found = new Dictionary<IntPtr, string>();
        EnumWindows((window, _) =>
        {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId && IsWindowVisible(window))
            {
                var name = new StringBuilder(256);
                var title = new StringBuilder(256);
                GetClassNameW(window, name, name.Capacity);
                GetWindowTextW(window, title, title.Capacity);
                found[window] = name + "|" + title;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    /// The text of a message box: its static control has the ID 0xFFFF.
    public static string MessageBoxText(IntPtr dialog)
    {
        var text = new StringBuilder(4096);
        GetDlgItemTextW(dialog, 0xFFFF, text, text.Capacity);
        return text.ToString();
    }
}
'@

# The window classes to tell apart: a message box is a dialog of the
# system's own class, and winit gives every window it makes this one.
$MessageBoxClass = '#32770'
$WinitWindowClass = 'Window Class'
# The main window's title starts with the app's name; an error box has it
# alone. Named apart from the titles read below, as PowerShell's variable
# names ignore case.
$AppName = 'Ruston Mail'
# WM_SYSCOMMAND with SC_CLOSE, as the title bar's close button sends.
$WM_SYSCOMMAND = 0x0112
$SC_CLOSE = 0xF060
# How often the window is looked for, how long it draws before the close,
# and how long the close may take.
$PollMilliseconds = 250
$DrawSeconds = 5
$CloseMilliseconds = 15000

# Each visible window of $Process: its handle, class and title.
function Get-VisibleWindows([System.Diagnostics.Process]$Process) {
    foreach ($entry in [RustonSmokeWindows]::VisibleWindowsOf([uint32]$Process.Id).GetEnumerator()) {
        $class, $windowTitle = $entry.Value -split '\|', 2
        [pscustomobject]@{ Handle = $entry.Key; Class = $class; Title = $windowTitle }
    }
}

# Fails the test, ending the process, when Ruston Mail shows an error box: a
# failure to start shows one rather than ending the process.
function Assert-NoErrorBox([System.Diagnostics.Process]$Process) {
    foreach ($window in Get-VisibleWindows $Process) {
        if ($window.Class -eq $MessageBoxClass -and $window.Title -eq $AppName) {
            $text = [RustonSmokeWindows]::MessageBoxText($window.Handle)
            Stop-Process -Id $Process.Id -Force
            throw "[smoke] Ruston Mail showed an error: $text"
        }
    }
}

# The handle of Ruston Mail's main window, or zero while it has none.
function Find-RustonWindow([System.Diagnostics.Process]$Process) {
    foreach ($window in Get-VisibleWindows $Process) {
        if ($window.Class -eq $WinitWindowClass -and $window.Title -like "$AppName*") {
            return $window.Handle
        }
    }
    return [IntPtr]::Zero
}

$log = Join-Path ([System.IO.Path]::GetTempPath()) "ruston-smoke-$PID.log"
$process = Start-Process -FilePath $Executable -PassThru -RedirectStandardError $log
# Opening the handle now keeps the exit status readable after the process ends.
[void]$process.Handle
try {
    $window = [IntPtr]::Zero
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ($window -eq [IntPtr]::Zero) {
        if ($process.HasExited) {
            throw "[smoke] $Executable exited with status $($process.ExitCode) before its window opened"
        }
        if ((Get-Date) -gt $deadline) {
            Stop-Process -Id $process.Id -Force
            throw "[smoke] $Executable opened no window within $Seconds s"
        }
        Start-Sleep -Milliseconds $PollMilliseconds
        Assert-NoErrorBox $process
        $window = Find-RustonWindow $process
    }

    # Let it draw for a while: a failing renderer would show its error box now.
    Start-Sleep -Seconds $DrawSeconds
    Assert-NoErrorBox $process
    if ($process.HasExited) {
        throw "[smoke] $Executable exited with status $($process.ExitCode) after opening its window"
    }

    [void][RustonSmokeWindows]::PostMessageW($window, $WM_SYSCOMMAND, [IntPtr]$SC_CLOSE, [IntPtr]::Zero)
    if (-not $process.WaitForExit($CloseMilliseconds)) {
        Stop-Process -Id $process.Id -Force
        throw "[smoke] $Executable did not close within $($CloseMilliseconds / 1000) s"
    }
    Write-Host "[smoke] $Executable opened its window, drew it and closed"
} finally {
    if (Test-Path -LiteralPath $log) {
        Get-Content -LiteralPath $log | ForEach-Object { Write-Host "[smoke] stderr: $_" }
        Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
    }
}
