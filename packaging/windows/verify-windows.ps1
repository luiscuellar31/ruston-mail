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
    // Where the PE format keeps what is read here, from Microsoft's
    // specification. The DOS header says where the PE header starts.
    const int PeHeaderPointerOffset = 0x3C;
    // The PE header starts with "PE\0\0", then the file header's fields.
    const uint PeSignature = 0x00004550;
    const int MachineOffset = 4;
    const int SectionCountOffset = 6;
    const int OptionalHeaderSizeOffset = 20;
    // The optional header follows the signature and the 20-byte file header.
    const int OptionalHeaderOffset = 24;
    // The optional header of a 64-bit image, the only kind built.
    const ushort Pe32PlusMagic = 0x20B;
    const int SubsystemOffset = 68;
    // Its data directories, eight bytes each; the second lists the imports.
    const int DataDirectoriesOffset = 112;
    const int DataDirectorySize = 8;
    const int ImportDirectoryIndex = 1;
    // One 20-byte descriptor per imported DLL; a zero name ends the list.
    const int ImportDescriptorSize = 20;
    const int ImportNameOffset = 12;
    // The section headers, 40 bytes each, after the optional header.
    const int SectionHeaderSize = 40;
    const int SectionVirtualSizeOffset = 8;
    const int SectionAddressOffset = 12;
    const int SectionRawDataOffset = 20;
    // Resource 1 of type RT_MANIFEST is what Windows reads at launch.
    const int ApplicationManifestId = 1;
    const int RtManifest = 24;
    // Open an image only to read it: never run it, and read one built for
    // another architecture too.
    const uint LoadLibraryAsDatafile = 0x2;
    const uint LoadLibraryAsImageResource = 0x20;

    public sealed class Image
    {
        public ushort Machine;
        public ushort Subsystem;
        public List<string> Imports = new List<string>();
    }

    public static Image Read(string path)
    {
        byte[] bytes = File.ReadAllBytes(path);
        int header = BitConverter.ToInt32(bytes, PeHeaderPointerOffset);
        if (BitConverter.ToUInt32(bytes, header) != PeSignature)
            throw new InvalidDataException(path + " is not a PE image");
        var image = new Image();
        image.Machine = BitConverter.ToUInt16(bytes, header + MachineOffset);
        int sectionCount = BitConverter.ToUInt16(bytes, header + SectionCountOffset);
        int optionalSize = BitConverter.ToUInt16(bytes, header + OptionalHeaderSizeOffset);
        int optional = header + OptionalHeaderOffset;
        if (BitConverter.ToUInt16(bytes, optional) != Pe32PlusMagic)
            throw new InvalidDataException(path + " is not a 64-bit image");
        image.Subsystem = BitConverter.ToUInt16(bytes, optional + SubsystemOffset);
        int sections = optional + optionalSize;

        int imports = optional + DataDirectoriesOffset + ImportDirectoryIndex * DataDirectorySize;
        uint importRva = BitConverter.ToUInt32(bytes, imports);
        if (importRva == 0) return image;
        for (int descriptor = Offset(bytes, sections, sectionCount, importRva); ; descriptor += ImportDescriptorSize)
        {
            uint nameRva = BitConverter.ToUInt32(bytes, descriptor + ImportNameOffset);
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
            int section = sections + i * SectionHeaderSize;
            uint size = BitConverter.ToUInt32(bytes, section + SectionVirtualSizeOffset);
            uint address = BitConverter.ToUInt32(bytes, section + SectionAddressOffset);
            uint raw = BitConverter.ToUInt32(bytes, section + SectionRawDataOffset);
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

    public static string Manifest(string path)
    {
        IntPtr module = LoadLibraryExW(path, IntPtr.Zero, LoadLibraryAsDatafile | LoadLibraryAsImageResource);
        if (module == IntPtr.Zero)
            throw new IOException("could not open " + path, Marshal.GetLastWin32Error());
        try
        {
            IntPtr resource = FindResourceW(module, (IntPtr)ApplicationManifestId, (IntPtr)RtManifest);
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
$version = & (Join-Path (Split-Path $PSScriptRoot -Parent) 'workspace-version.ps1')

$machines = @{ 'x86_64' = 0x8664; 'arm64' = 0xAA64 }
$executables = @(
    @{ Name = 'ruston.exe'; Subsystem = 2; Kind = 'window app'; Description = 'Ruston Mail'
       Cargo = 'Cargo.toml'; Settings = @('dpiAwareness', 'longPathAware', 'activeCodePage') },
    @{ Name = 'ruston-cli.exe'; Subsystem = 3; Kind = 'console program'; Description = 'Ruston Mail CLI'
       Cargo = 'crates\ruston-cli\Cargo.toml'; Settings = @('longPathAware', 'activeCodePage') }
)
# The C runtime is linked statically; these DLLs are not on a clean Windows.
$runtimeLibraries = '^(vcruntime|msvcp|concrt|vccorlib|ucrtbase|api-ms-win-crt-)'
# The supportedOS a manifest gives for Windows 10 and 11.
$windows10Id = '{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}'
# The value each manifest setting an executable lists must have.
$manifestValues = @{ dpiAwareness = 'PerMonitorV2'; longPathAware = 'true'; activeCodePage = 'UTF-8' }

# What it is built for: the architecture asked for.
function Assert-Machine($Executable, $Image) {
    if ($Image.Machine -ne $machines[$Architecture]) {
        Stop-Verification ('{0} is built for machine 0x{1:X4}, expected {2}' -f $Executable.Name, $Image.Machine, $Architecture)
    }
}

# The kind of program it is: a window app or a console program.
function Assert-Subsystem($Executable, $Image) {
    if ($Image.Subsystem -ne $Executable.Subsystem) {
        Stop-Verification "$($Executable.Name) is not a $($Executable.Kind) (subsystem $($Image.Subsystem))"
    }
}

# The DLLs it loads: Windows' own, never the C runtime's.
function Assert-Imports($Executable, $Image) {
    foreach ($library in $Image.Imports) {
        if ($library -match $runtimeLibraries) {
            Stop-Verification "$($Executable.Name) needs the C runtime DLL $library"
        }
        $apiSet = $library -match '^(api|ext)-ms-'
        if (-not $apiSet -and -not (Test-Path -LiteralPath (Join-Path $env:SystemRoot "System32\$library"))) {
            Stop-Verification "$($Executable.Name) loads $library, which Windows does not include"
        }
    }
}

# The manifest Windows reads: run as the user, Windows 10 and 11, and the
# executable's settings.
function Assert-Manifest($Executable, [string]$Path) {
    $text = [RustonPortableExecutable]::Manifest($Path)
    if (-not $text) { Stop-Verification "$($Executable.Name) has no embedded manifest" }
    $manifest = [xml]$text
    $level = $manifest.GetElementsByTagName('requestedExecutionLevel') | Select-Object -First 1
    if (-not $level -or $level.level -ne 'asInvoker') {
        Stop-Verification "$($Executable.Name) does not run as the invoking user"
    }
    $supported = $manifest.GetElementsByTagName('supportedOS') | Where-Object { $_.Id -eq $windows10Id }
    if (-not $supported) { Stop-Verification "$($Executable.Name) does not declare Windows 10 and 11" }
    foreach ($setting in $Executable.Settings) {
        $element = $manifest.GetElementsByTagName($setting) | Select-Object -First 1
        if (-not $element -or $element.InnerText -ne $manifestValues[$setting]) {
            Stop-Verification "$($Executable.Name) manifest lacks $setting $($manifestValues[$setting])"
        }
    }
}

# The version information: its package's name, version and description.
function Assert-VersionInfo($Executable, [string]$Path) {
    $info = (Get-Item -LiteralPath $Path).VersionInfo
    $description = Get-CargoField (Join-Path $repository $Executable.Cargo) 'description'
    if ($info.ProductName -ne 'Ruston Mail' -or
        $info.ProductVersion -ne $version -or
        $info.FileDescription -ne $Executable.Description -or
        $info.Comments -ne $description -or
        $info.OriginalFilename -ne $Executable.Name) {
        Stop-Verification "$($Executable.Name) version information does not match its package ($version)"
    }
}

foreach ($executable in $executables) {
    $path = Join-Path $Directory $executable.Name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { Stop-Verification "Missing $path" }
    Write-Step $executable.Name
    $fullPath = (Resolve-Path -LiteralPath $path).ProviderPath
    $image = [RustonPortableExecutable]::Read($fullPath)
    Assert-Machine $executable $image
    Assert-Subsystem $executable $image
    Assert-Imports $executable $image
    Assert-Manifest $executable $fullPath
    Assert-VersionInfo $executable $path
}

# An x64 system cannot run ARM64 programs; an ARM64 one runs both.
$os = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($Architecture -eq 'arm64' -and $os -ne 'Arm64') {
    Write-Step "ruston-cli --version not run: $Architecture build on a $os system"
} else {
    Write-Step 'ruston-cli --version'
    $reported = & (Join-Path $Directory 'ruston-cli.exe') --version
    if ($LASTEXITCODE -ne 0 -or $reported -ne "ruston-cli $version") {
        Stop-Verification "ruston-cli reports '$reported', expected 'ruston-cli $version'"
    }
}

Write-Step "$Directory is ready: $Architecture, version $version"
