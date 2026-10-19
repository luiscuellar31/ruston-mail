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
    public static Dictionary<IntPtr, string> Of(uint processId)
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

function Find-RustonWindows([System.Diagnostics.Process]$Process) {
    $windows = [RustonSmokeWindows]::Of([uint32]$Process.Id)
    foreach ($entry in $windows.GetEnumerator()) {
        $class, $title = $entry.Value -split '\|', 2
        if ($class -eq '#32770' -and $title -eq 'Ruston Mail') {
            $text = [RustonSmokeWindows]::MessageBoxText($entry.Key)
            Stop-Process -Id $Process.Id -Force
            throw "[smoke] Ruston Mail showed an error: $text"
        }
        if ($class -eq 'Window Class' -and $title -like 'Ruston Mail*') {
            return $entry.Key
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
        Start-Sleep -Milliseconds 250
        $window = Find-RustonWindows $process
    }

    # Let it draw for a while: a failing renderer would show its error box now.
    Start-Sleep -Seconds 5
    [void](Find-RustonWindows $process)
    if ($process.HasExited) {
        throw "[smoke] $Executable exited with status $($process.ExitCode) after opening its window"
    }

    # WM_SYSCOMMAND with SC_CLOSE, as the title bar's close button sends.
    [void][RustonSmokeWindows]::PostMessageW($window, 0x0112, [IntPtr]0xF060, [IntPtr]::Zero)
    if (-not $process.WaitForExit(15000)) {
        Stop-Process -Id $process.Id -Force
        throw "[smoke] $Executable did not close within 15 s"
    }
    Write-Host "[smoke] $Executable opened its window, drew it and closed"
} finally {
    if (Test-Path -LiteralPath $log) {
        Get-Content -LiteralPath $log | ForEach-Object { Write-Host "[smoke] stderr: $_" }
        Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
    }
}
