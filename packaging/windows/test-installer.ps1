# ==============================================================================
# Ruston Mail - Windows installer test
# Installs silently for the current user with email-link registration, checks
# what the installation adds, starts the installed app, uninstalls silently,
# and checks that only user data is left. Other apps' registrations must stay.
# Meant for disposable CI runners: it installs into the real user profile.
# ==============================================================================
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Installer
)

$ErrorActionPreference = 'Stop'

function Write-Step([string]$Message) { Write-Host "[installer] $Message" }
function Stop-Test([string]$Message) { throw "[FAIL] $Message" }

$app = Join-Path $env:LOCALAPPDATA 'Programs\Ruston Mail'
$programs = [Environment]::GetFolderPath('Programs')
$shortcut = Join-Path $programs 'Ruston Mail.lnk'
$progId = 'HKCU:\Software\Classes\com.luiscuellar.ruston-mail.mailto'
$apps = 'HKCU:\Software\RegisteredApplications'
if (-not (Test-Path $apps)) { New-Item -Path $apps | Out-Null }
New-ItemProperty -Path $apps -Name 'Other app' -Value 'Software\Other\Capabilities' -PropertyType String -Force | Out-Null

Write-Step 'Installing'
$setupLog = Join-Path ([System.IO.Path]::GetTempPath()) 'ruston-setup.log'
$setup = Start-Process -FilePath $Installer -Wait -PassThru -ArgumentList `
    '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/TASKS=mailto', "/LOG=`"$setupLog`""
if ($setup.ExitCode -ne 0) {
    Get-Content -LiteralPath $setupLog -ErrorAction SilentlyContinue | Select-Object -Last 30 | Write-Host
    Stop-Test "Setup exited with status $($setup.ExitCode)"
}

foreach ($file in 'ruston.exe', 'ruston-cli.exe', 'LICENSE.txt', 'unins000.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $app $file))) { Stop-Test "Missing $app\$file" }
}
if (-not (Test-Path -LiteralPath $shortcut)) { Stop-Test "Missing $shortcut" }
$shell = New-Object -ComObject Shell.Application
$link = $shell.Namespace($programs).ParseName('Ruston Mail.lnk')
$appId = $link.ExtendedProperty('System.AppUserModel.ID')
if ($appId -ne 'com.luiscuellar.ruston-mail') { Stop-Test "The shortcut carries app ID '$appId'" }
$target = $link.GetLink.Path
if ($target -ne (Join-Path $app 'ruston.exe')) { Stop-Test "The shortcut points at '$target'" }

$command = (Get-Item "$progId\shell\open\command").GetValue('')
if ($command -ne ('"{0}" "%1"' -f (Join-Path $app 'ruston.exe'))) { Stop-Test "The mailto command is '$command'" }
if ((Get-ItemPropertyValue $apps -Name 'Ruston Mail') -ne 'Software\Ruston Mail\Capabilities') {
    Stop-Test 'Ruston Mail is not a registered application'
}

$cli = & (Join-Path $app 'ruston-cli.exe') --version
Write-Step "Installed: $cli"

Write-Step 'Starting the installed app'
& (Join-Path $PSScriptRoot 'smoke-test.ps1') -Executable (Join-Path $app 'ruston.exe')

Write-Step 'Uninstalling'
# The uninstaller reruns itself from a temporary copy and returns at once, so
# wait for it to take the program folder away.
Start-Process -FilePath (Join-Path $app 'unins000.exe') -Wait -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
$deadline = (Get-Date).AddSeconds(60)
while ((Test-Path -LiteralPath $app) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
if (Test-Path -LiteralPath $app) { Stop-Test "$app is still there after uninstalling" }
if (Test-Path -LiteralPath $shortcut) { Stop-Test 'The Start menu shortcut is still there' }
if (Test-Path $progId) { Stop-Test 'The mailto handler is still registered' }
if (Test-Path 'HKCU:\Software\Ruston Mail') { Stop-Test 'The capabilities are still registered' }
if ((Get-Item $apps).Property -contains 'Ruston Mail') { Stop-Test 'Ruston Mail is still a registered application' }
if ((Get-ItemPropertyValue $apps -Name 'Other app') -ne 'Software\Other\Capabilities') {
    Stop-Test 'Another app lost its registration'
}
Remove-ItemProperty -Path $apps -Name 'Other app'

Write-Step "$Installer installs, starts and uninstalls cleanly"
