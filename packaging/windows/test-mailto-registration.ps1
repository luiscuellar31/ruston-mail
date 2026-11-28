# ==============================================================================
# Ruston Mail - Windows mailto registration test
# Registers a copy of ruston.exe from a folder with a space in its path, twice,
# as a reinstall would, and checks its quoted command, its capability and its
# entry among the registered applications; then unregisters it and checks that
# all of that is gone. Another app's entry and the user's default email app
# must stay as they were throughout.
# Meant for disposable CI runners: it writes the real user's registry.
# ==============================================================================
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Executable
)

$ErrorActionPreference = 'Stop'

function Write-Step([string]$Message) { Write-Host "[mailto] $Message" }
function Stop-Test([string]$Message) { throw "[FAIL] $Message" }

# The user's default email app, which registering must never change: Windows
# protects it, and only the user picks it, in Settings.
function Get-MailtoUserChoice {
    $choice = 'HKCU:\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\mailto\UserChoice'
    if (Test-Path $choice) { Get-ItemProperty $choice | ConvertTo-Json } else { '' }
}

$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$folder = Join-Path $temp 'Ruston mailto test'
New-Item -ItemType Directory -Path $folder -Force | Out-Null
$copy = Join-Path $folder 'ruston.exe'
Copy-Item -LiteralPath $Executable -Destination $copy
$mailtoClassKey = 'HKCU:\Software\Classes\com.luiscuellar.ruston-mail.mailto'
$rustonKey = 'HKCU:\Software\Ruston Mail'
$apps = 'HKCU:\Software\RegisteredApplications'
$before = Get-MailtoUserChoice
# Other apps' registrations share this key and must survive.
if (-not (Test-Path $apps)) { New-Item -Path $apps | Out-Null }
New-ItemProperty -Path $apps -Name 'Other app' -Value 'Software\Other\Capabilities' -PropertyType String -Force | Out-Null

Write-Step 'Registering, twice, as a reinstall would'
$register = Join-Path $PSScriptRoot 'register-mailto.ps1'
& $register -ExecutablePath $copy
& $register -ExecutablePath $copy
$command = (Get-Item "$mailtoClassKey\shell\open\command").GetValue('')
if ($command -ne ('"{0}" "%1"' -f $copy)) { Stop-Test 'Executable path was not quoted correctly.' }
$handler = Get-ItemPropertyValue "$rustonKey\Capabilities\UrlAssociations" -Name mailto
if ($handler -ne 'com.luiscuellar.ruston-mail.mailto') { Stop-Test 'MAILTO capability was not registered.' }
$registered = Get-ItemPropertyValue $apps -Name 'Ruston Mail'
if ($registered -ne 'Software\Ruston Mail\Capabilities') { Stop-Test 'Application registration is missing.' }
if ((Get-ItemPropertyValue $apps -Name 'Other app') -ne 'Software\Other\Capabilities') { Stop-Test 'Registration removed another app.' }
if ((Get-MailtoUserChoice) -ne $before) { Stop-Test 'Registration changed the default MAILTO association.' }

Write-Step 'Unregistering'
& (Join-Path $PSScriptRoot 'unregister-mailto.ps1')
if (Test-Path $mailtoClassKey) { Stop-Test 'The mailto handler is still registered.' }
if (Test-Path $rustonKey) { Stop-Test 'The capabilities are still registered.' }
if ((Get-Item $apps).Property -contains 'Ruston Mail') { Stop-Test 'The application is still registered.' }
if ((Get-ItemPropertyValue $apps -Name 'Other app') -ne 'Software\Other\Capabilities') { Stop-Test 'Unregistration removed another app.' }
if ((Get-MailtoUserChoice) -ne $before) { Stop-Test 'Unregistration changed the default MAILTO association.' }
Remove-ItemProperty -Path $apps -Name 'Other app'

Write-Step "$Executable registers and unregisters cleanly"
