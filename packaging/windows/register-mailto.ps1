# Register Ruston Mail as a selectable mailto handler for the current user.
# This does not change the default app or write Windows' protected UserChoice.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ExecutablePath
)

$ErrorActionPreference = 'Stop'
$executable = (Resolve-Path -LiteralPath $ExecutablePath).ProviderPath
if (-not (Test-Path -LiteralPath $executable -PathType Leaf) -or
    [System.IO.Path]::GetExtension($executable) -ne '.exe') {
    throw 'ExecutablePath must point to the installed Ruston Mail executable.'
}

$programId = 'com.luiscuellar.ruston-mail.mailto'
$classPath = "HKCU:\Software\Classes\$programId"
$capabilities = 'Software\Ruston Mail\Capabilities'

# New-Item -Force recreates an existing key empty, and RegisteredApplications
# holds every other app's entry, so create a key only when it is missing.
function New-KeyIfMissing([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -Path $Path -Force | Out-Null
    }
}

New-KeyIfMissing "$classPath\shell\open\command"
Set-Item -Path $classPath -Value 'Ruston Mail email link'
New-ItemProperty -Path $classPath -Name 'URL Protocol' -Value '' -PropertyType String -Force | Out-Null
Set-Item -Path "$classPath\shell\open\command" -Value ('"{0}" "%1"' -f $executable)
New-KeyIfMissing "$classPath\DefaultIcon"
Set-Item -Path "$classPath\DefaultIcon" -Value ('"{0}",0' -f $executable)

New-KeyIfMissing "HKCU:\$capabilities\UrlAssociations"
New-ItemProperty -Path "HKCU:\$capabilities" -Name 'ApplicationName' -Value 'Ruston Mail' -PropertyType String -Force | Out-Null
New-ItemProperty -Path "HKCU:\$capabilities" -Name 'ApplicationDescription' -Value 'Unofficial native desktop client for Proton Mail' -PropertyType String -Force | Out-Null
New-ItemProperty -Path "HKCU:\$capabilities\UrlAssociations" -Name 'mailto' -Value $programId -PropertyType String -Force | Out-Null
New-KeyIfMissing 'HKCU:\Software\RegisteredApplications'
New-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'Ruston Mail' -Value $capabilities -PropertyType String -Force | Out-Null

Write-Output 'Ruston Mail is available in Settings > Apps > Default apps. Select it for MAILTO if desired.'
