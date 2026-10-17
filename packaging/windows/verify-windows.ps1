# ==============================================================================
# Ruston Mail - Windows executable verification
# Checks ruston.exe and ruston-cli.exe before they ship: what they are built
# for, the kind of program each is, the DLLs they load, the manifest Windows
# reads, and the version they report.
# ==============================================================================
[CmdletBinding()]
param(
    # The folder holding ruston.exe and ruston-cli.exe.
    [Parameter(Mandatory = $true)]
    [string]$Directory,

    [Parameter(Mandatory = $true)]
    [ValidateSet('x86_64', 'arm64')]
    [string]$Architecture
)

$ErrorActionPreference = 'Stop'

function Write-Step([string]$Message) { Write-Host "[verify] $Message" }
function Stop-Verification([string]$Message) { throw "[FAIL] $Message" }

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public static class RustonPortableExecutable
{
    public sealed class Image
    {
        public ushort Machine;
        public ushort Subsystem;
        public List<string> Imports = new List<string>();
    }

    public static Image Read(string path)
    {
        byte[] bytes = File.ReadAllBytes(path);
        int header = BitConverter.ToInt32(bytes, 0x3C);
        if (BitConverter.ToUInt32(bytes, header) != 0x00004550)
            throw new InvalidDataException(path + " is not a PE image");
        var image = new Image();
        image.Machine = BitConverter.ToUInt16(bytes, header + 4);
        int sectionCount = BitConverter.ToUInt16(bytes, header + 6);
        int optionalSize = BitConverter.ToUInt16(bytes, header + 20);
        int optional = header + 24;
        if (BitConverter.ToUInt16(bytes, optional) != 0x20B)
            throw new InvalidDataException(path + " is not a 64-bit image");
        image.Subsystem = BitConverter.ToUInt16(bytes, optional + 68);
        int sections = optional + optionalSize;

        // Data directory 1 lists the imported DLLs, one 20-byte descriptor each.
        uint importRva = BitConverter.ToUInt32(bytes, optional + 112 + 8);
        if (importRva == 0) return image;
        for (int descriptor = Offset(bytes, sections, sectionCount, importRva); ; descriptor += 20)
        {
            uint nameRva = BitConverter.ToUInt32(bytes, descriptor + 12);
            if (nameRva == 0) break;
            int name = Offset(bytes, sections, sectionCount, nameRva);
            int end = Array.IndexOf(bytes, (byte)0, name);
            image.Imports.Add(Encoding.ASCII.GetString(bytes, name, end - name));
        }
        return image;
    }

    static int Offset(byte[] bytes, int sections, int count, uint rva)
    {
        for (int i = 0; i < count; i++)
        {
            int section = sections + i * 40;
            uint size = BitConverter.ToUInt32(bytes, section + 8);
            uint address = BitConverter.ToUInt32(bytes, section + 12);
            uint raw = BitConverter.ToUInt32(bytes, section + 20);
            if (rva >= address && rva < address + size)
                return (int)(rva - address + raw);
        }
        throw new InvalidDataException("address outside every section");
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
    [DllImport("kernel32.dll")]
    static extern bool FreeLibrary(IntPtr module);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr FindResourceW(IntPtr module, IntPtr name, IntPtr type);
    [DllImport("kernel32.dll")]
    static extern uint SizeofResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll")]
    static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll")]
    static extern IntPtr LockResource(IntPtr data);

    // Resource 1 of type RT_MANIFEST (24) is what Windows reads at launch.
    public static string Manifest(string path)
    {
        // LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE: never run
        // the image, and read images built for another architecture too.
        IntPtr module = LoadLibraryExW(path, IntPtr.Zero, 0x2 | 0x20);
        if (module == IntPtr.Zero)
            throw new IOException("could not open " + path, Marshal.GetLastWin32Error());
        try
        {
            IntPtr resource = FindResourceW(module, (IntPtr)1, (IntPtr)24);
            if (resource == IntPtr.Zero) return null;
            int size = (int)SizeofResource(module, resource);
            byte[] data = new byte[size];
            Marshal.Copy(LockResource(LoadResource(module, resource)), data, 0, size);
            return Encoding.UTF8.GetString(data);
        }
        finally { FreeLibrary(module); }
    }
}
'@

$repository = Resolve-Path (Join-Path $PSScriptRoot '..\..')
function Get-CargoField([string]$Manifest, [string]$Field) {
    $line = Get-Content -LiteralPath $Manifest | Where-Object { $_ -match "^$Field = `"" } | Select-Object -First 1
    if (-not $line) { Stop-Verification "No $Field in $Manifest" }
    return ($line -split '"')[1]
}
$version = Get-CargoField (Join-Path $repository 'Cargo.toml') 'version'

$machines = @{ 'x86_64' = 0x8664; 'arm64' = 0xAA64 }
$executables = @(
    @{ Name = 'ruston.exe'; Subsystem = 2; Kind = 'window app'; Description = 'Ruston Mail'
       Cargo = 'Cargo.toml'; Settings = @('dpiAwareness', 'longPathAware', 'activeCodePage') },
    @{ Name = 'ruston-cli.exe'; Subsystem = 3; Kind = 'console program'; Description = 'Ruston Mail CLI'
       Cargo = 'crates\ruston-cli\Cargo.toml'; Settings = @('longPathAware', 'activeCodePage') }
)
# The C runtime is linked statically; these DLLs are not on a clean Windows.
$runtimeLibraries = '^(vcruntime|msvcp|concrt|vccorlib|ucrtbase|api-ms-win-crt-)'

foreach ($executable in $executables) {
    $path = Join-Path $Directory $executable.Name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { Stop-Verification "Missing $path" }
    Write-Step $executable.Name
    $image = [RustonPortableExecutable]::Read((Resolve-Path -LiteralPath $path).ProviderPath)

    if ($image.Machine -ne $machines[$Architecture]) {
        Stop-Verification ('{0} is built for machine 0x{1:X4}, expected {2}' -f $executable.Name, $image.Machine, $Architecture)
    }
    if ($image.Subsystem -ne $executable.Subsystem) {
        Stop-Verification "$($executable.Name) is not a $($executable.Kind) (subsystem $($image.Subsystem))"
    }

    foreach ($library in $image.Imports) {
        if ($library -match $runtimeLibraries) {
            Stop-Verification "$($executable.Name) needs the C runtime DLL $library"
        }
        $apiSet = $library -match '^(api|ext)-ms-'
        if (-not $apiSet -and -not (Test-Path -LiteralPath (Join-Path $env:SystemRoot "System32\$library"))) {
            Stop-Verification "$($executable.Name) loads $library, which Windows does not include"
        }
    }

    $text = [RustonPortableExecutable]::Manifest((Resolve-Path -LiteralPath $path).ProviderPath)
    if (-not $text) { Stop-Verification "$($executable.Name) has no embedded manifest" }
    $manifest = [xml]$text
    $level = $manifest.GetElementsByTagName('requestedExecutionLevel') | Select-Object -First 1
    if (-not $level -or $level.level -ne 'asInvoker') {
        Stop-Verification "$($executable.Name) does not run as the invoking user"
    }
    $supported = $manifest.GetElementsByTagName('supportedOS') | Where-Object { $_.Id -eq '{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}' }
    if (-not $supported) { Stop-Verification "$($executable.Name) does not declare Windows 10 and 11" }
    $expected = @{ dpiAwareness = 'PerMonitorV2'; longPathAware = 'true'; activeCodePage = 'UTF-8' }
    foreach ($setting in $executable.Settings) {
        $element = $manifest.GetElementsByTagName($setting) | Select-Object -First 1
        if (-not $element -or $element.InnerText -ne $expected[$setting]) {
            Stop-Verification "$($executable.Name) manifest lacks $setting $($expected[$setting])"
        }
    }

    $info = (Get-Item -LiteralPath $path).VersionInfo
    $description = Get-CargoField (Join-Path $repository $executable.Cargo) 'description'
    if ($info.ProductName -ne 'Ruston Mail' -or
        $info.ProductVersion -ne $version -or
        $info.FileDescription -ne $executable.Description -or
        $info.Comments -ne $description -or
        $info.OriginalFilename -ne $executable.Name) {
        Stop-Verification "$($executable.Name) version information does not match its package ($version)"
    }
}

Write-Step 'ruston-cli --version'
$reported = & (Join-Path $Directory 'ruston-cli.exe') --version
if ($LASTEXITCODE -ne 0 -or $reported -ne "ruston-cli $version") {
    Stop-Verification "ruston-cli reports '$reported', expected 'ruston-cli $version'"
}

Write-Step "$Directory is ready: $Architecture, version $version"
