# ==============================================================================
# Ruston Mail - Windows packaging
# Builds ruston.exe and ruston-cli.exe for one architecture from the lockfile,
# checks them, and writes a per-user installer and a portable ZIP to dist/,
# each with a .sha256 file holding its hash.
# ==============================================================================
[CmdletBinding()]
param(
    [ValidateSet('x86_64', 'arm64')]
    [string]$Architecture = $(if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x86_64' }),

    # Where the installer and the ZIP go.
    [string]$Dist,

    # ISCC.exe from Inno Setup 6; looked for in its usual places when omitted.
    [string]$InnoSetup,

    # Package the binaries already in target/<triple>/release.
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

function Write-Step([string]$Message) { Write-Host "[package] $Message" }

$repository = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).ProviderPath
if (-not $Dist) { $Dist = Join-Path $repository 'dist' }
$target = @{ x86_64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Architecture]
$versionLine = Get-Content -LiteralPath (Join-Path $repository 'Cargo.toml') |
    Where-Object { $_ -match '^version = "' } | Select-Object -First 1
$version = $versionLine.Split('"')[1]

if (-not $InnoSetup) {
    $InnoSetup = @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    ) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $InnoSetup) { throw 'Inno Setup 6 not found; pass -InnoSetup <path to ISCC.exe>' }
}

if (-not $SkipBuild) {
    Write-Step "Building $target"
    Push-Location $repository
    try {
        cargo build --locked --release --target $target -p ruston-mail -p ruston-cli --bins
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    } finally {
        Pop-Location
    }
}
$binaries = Join-Path $repository "target\$target\release"
& (Join-Path $PSScriptRoot 'verify-windows.ps1') -Directory $binaries -Architecture $Architecture

$name = "ruston-mail-$version-$Architecture"
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
$Dist = (Resolve-Path -LiteralPath $Dist).ProviderPath

Write-Step 'Staging the portable folder'
$stage = Join-Path $Dist "$name-windows"
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item -LiteralPath (Join-Path $binaries 'ruston.exe'), (Join-Path $binaries 'ruston-cli.exe') -Destination $stage
Copy-Item -LiteralPath (Join-Path $repository 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt')
foreach ($file in 'README.txt', 'register-mailto.ps1', 'unregister-mailto.ps1') {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $stage
}

Write-Step 'Writing the ZIP'
$zip = Join-Path $Dist "$name-windows-UNSIGNED.zip"
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
# Entries sit in one folder, so extracting gives that folder, not loose files.
# Names use '/', as the ZIP format requires: Windows PowerShell's .NET would
# write '\' if left to name them.
$archive = [System.IO.Compression.ZipFile]::Open($zip, 'Create')
try {
    foreach ($file in Get-ChildItem -LiteralPath $stage -File) {
        [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $archive, $file.FullName, "$name-windows/$($file.Name)", 'Optimal')
    }
} finally {
    $archive.Dispose()
}

Write-Step 'Compiling the installer'
& $InnoSetup /Q "/DVersion=$version" "/DArch=$Architecture" "/DSourceDir=$stage" "/DOutputDir=$Dist" (Join-Path $PSScriptRoot 'ruston-mail.iss')
if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
$setup = Join-Path $Dist "$name-setup-UNSIGNED.exe"
if (-not (Test-Path -LiteralPath $setup)) { throw "Missing $setup" }

foreach ($file in $setup, $zip) {
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText("$file.sha256", "$hash`n")
    Write-Step "$(Split-Path -Leaf $file)  $hash"
}
