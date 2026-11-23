# Prints the version in [workspace.package] of the root Cargo.toml, the one
# every crate and every release artifact takes, as workspace-version.sh does.
# Only that table is read: the first `version = ` of the file need not be its
# own.
$ErrorActionPreference = 'Stop'

$manifest = Join-Path (Split-Path $PSScriptRoot -Parent) 'Cargo.toml'
$inPackage = $false
foreach ($line in Get-Content -LiteralPath $manifest) {
    if ($line -match '^\[') {
        $inPackage = $line -eq '[workspace.package]'
        continue
    }
    if ($inPackage -and $line -match '^version = "([^"]*)"$') {
        return $Matches[1]
    }
}
throw "No version in [workspace.package] of $manifest"
