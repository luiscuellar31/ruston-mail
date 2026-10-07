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

New-Item -Path "$classPath\shell\open\command" -Force | Out-Null
Set-Item -Path $classPath -Value 'Ruston Mail email link'
New-ItemProperty -Path $classPath -Name 'URL Protocol' -Value '' -PropertyType String -Force | Out-Null
Set-Item -Path "$classPath\shell\open\command" -Value ('"{0}" "%1"' -f $executable)
New-Item -Path "$classPath\DefaultIcon" -Force | Out-Null
Set-Item -Path "$classPath\DefaultIcon" -Value ('"{0}",0' -f $executable)

New-Item -Path "HKCU:\$capabilities\UrlAssociations" -Force | Out-Null
New-ItemProperty -Path "HKCU:\$capabilities" -Name 'ApplicationName' -Value 'Ruston Mail' -PropertyType String -Force | Out-Null
New-ItemProperty -Path "HKCU:\$capabilities" -Name 'ApplicationDescription' -Value 'Unofficial native desktop client for Proton Mail' -PropertyType String -Force | Out-Null
New-ItemProperty -Path "HKCU:\$capabilities\UrlAssociations" -Name 'mailto' -Value $programId -PropertyType String -Force | Out-Null
New-Item -Path 'HKCU:\Software\RegisteredApplications' -Force | Out-Null
New-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'Ruston Mail' -Value $capabilities -PropertyType String -Force | Out-Null

Write-Output 'Ruston Mail is available in Settings > Apps > Default apps. Select it for MAILTO if desired.'
