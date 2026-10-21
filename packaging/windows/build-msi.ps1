# Builds the Windows installer from release ae3.exe and ae3-gui.exe.
#
#   packaging/windows/build-msi.ps1 -Version <x.y.z> -BinDir <dir> -OutDir <dir>
#
# Writes <OutDir>/swgoh-ae3-windows-x86_64.msi and its .sha256. Needs the WiX
# Toolset 5 command line (dotnet tool install --global wix --version 5.0.2).
# Run from the repository root, after THIRD-PARTY-LICENSES.md is generated.
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$BinDir,
    [Parameter(Mandatory)][string]$OutDir
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$name = 'swgoh-ae3-windows-x86_64'
# MSI versions are numeric: 0.2.0-rc.1 installs as 0.2.0.
$msiVersion = $Version -replace '[-+].*$', ''

New-Item -ItemType Directory -Force $OutDir | Out-Null
$out = (Resolve-Path $OutDir).Path
$docs = Join-Path ([IO.Path]::GetTempPath()) "swgoh-ae3-msi-$PID"
New-Item -ItemType Directory -Force $docs | Out-Null
try {
    Copy-Item -Destination $docs -Path @(
        'LICENSE', 'README.md', 'CHANGELOG.md', 'THIRD-PARTY-LICENSES.md',
        'crates/ae3-audio/THIRD-PARTY-NOTICES.md', 'crates/ae3-gui/FONT-NOTICES.md'
    )
    $msi = Join-Path $out "$name.msi"
    wix build packaging/windows/swgoh-ae3.wxs -arch x64 `
        -d "Version=$msiVersion" `
        -d "BinDir=$((Resolve-Path $BinDir).Path)" `
        -d "DocDir=$docs" `
        -d "IconFile=$((Resolve-Path packaging/icons/swgoh-ae3.ico).Path)" `
        -o $msi
    if ($LASTEXITCODE -ne 0) { throw "wix build failed with exit code $LASTEXITCODE" }
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $out "$name.wixpdb")

    # The same "<hash> *<file>" line as the other archives' .sha256 files.
    $hash = (Get-FileHash -Algorithm SHA256 $msi).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$msi.sha256", "$hash *$name.msi`n")
    Write-Host "wrote $msi"
}
finally {
    Remove-Item -Recurse -Force $docs
}
