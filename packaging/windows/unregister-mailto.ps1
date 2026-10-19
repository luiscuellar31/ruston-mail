# Remove what register-mailto.ps1 added for the current user. Other apps'
# registrations and Windows' protected UserChoice are left alone: if Ruston
# Mail was the default email app, Windows asks for another one at the next
# link.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$programId = 'com.luiscuellar.ruston-mail.mailto'
foreach ($key in "HKCU:\Software\Classes\$programId", 'HKCU:\Software\Ruston Mail\Capabilities') {
    if (Test-Path -LiteralPath $key) {
        Remove-Item -LiteralPath $key -Recurse
    }
}

# Ruston Mail writes nothing else there; keep the key if something else does.
$product = 'HKCU:\Software\Ruston Mail'
if ((Test-Path -LiteralPath $product) -and
    -not (Get-ChildItem -LiteralPath $product) -and
    -not (Get-Item -LiteralPath $product).Property) {
    Remove-Item -LiteralPath $product
}

# Only Ruston Mail's value: the key lists every other registered app too.
$registered = 'HKCU:\Software\RegisteredApplications'
if ((Test-Path -LiteralPath $registered) -and
    (Get-Item -LiteralPath $registered).Property -contains 'Ruston Mail') {
    Remove-ItemProperty -LiteralPath $registered -Name 'Ruston Mail'
}

Write-Output 'Ruston Mail is no longer offered for MAILTO in Settings > Apps > Default apps.'
