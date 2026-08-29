# Package the Windows build into a self-contained MSIX that grants package
# identity at launch, sign it, and optionally install it.
#
# Usage:
#   powershell -File scripts/package-msix.ps1 -Profile release
#   powershell -File scripts/package-msix.ps1 -Profile release -Install
#
# Output: target/{profile}/Resolved.msix
#
# Prerequisites: MakeAppx.exe + SignTool.exe (Windows SDK) and a self-signed
# code-signing certificate whose subject CN matches the manifest Publisher.
# The script creates the certificate in Cert:\CurrentUser\My on first use and
# trusts it for the current user; installing the package machine-wide for all
# users needs admin (Local Machine Trusted People).
#
# Why MSIX: Windows denies package identity to bare executables, and several
# capabilities Resolved relies on (app identity for WebView2 data folders,
# fail-fast process identity, future Store distribution) come from that
# identity. The desktop app therefore runs as an MSIX-packaged exe, mirroring
# the macOS requirement that it runs from Resolved.app.

param(
    [ValidateSet('debug', 'release')]
    [string]$Profile = 'debug',
    [switch]$Install,
    [switch]$InstallCert
)

$ErrorActionPreference = 'Stop'

$projectDir = Split-Path -Parent $PSScriptRoot
$exePath = Join-Path $projectDir "target\$Profile\api-tester.exe"
if (-not (Test-Path $exePath)) {
    throw "missing $exePath - run 'scripts/cargo.ps1 build' first"
}

$version = (& cargo metadata --no-deps --format-version 1 |
    ConvertFrom-Json).packages[0].version
# MSIX versions must be four numeric parts ending in .0 (`x.y.z.w`).
$manifestVersion = "$version.0"

$identityName = 'nous.resolved'
$certificateSubject = 'CN=Nous Research'
$publisher = $certificateSubject.Substring(3)

function Find-SdkTool([string]$name) {
    $versions = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" `
        -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^10\.' } |
        Sort-Object Name -Descending
    foreach ($version in $versions) {
        $tool = Join-Path $version.FullName "x64\$name"
        if (Test-Path $tool) { return (Get-Item $tool) }
    }
    return $null
}

if ($InstallCert) {
    $existing = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $certificateSubject -and $_.HasPrivateKey } |
        Sort-Object NotBefore -Descending | Select-Object -First 1
    if ($existing) {
        Write-Host "Certificate already exists: $($existing.Thumbprint)"
    } else {
        $cert = New-SelfSignedCertificate -Type Custom -Subject $certificateSubject `
            -KeyUsage DigitalSignature -FriendlyName 'Resolved MSIX signing' `
            -CertStoreLocation 'Cert:\CurrentUser\My' `
            -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
        Write-Host "Created signing certificate $($cert.Thumbprint)"
    }
    return
}

$makeAppx = Find-SdkTool 'MakeAppx.exe'
if (-not $makeAppx) {
    throw 'MakeAppx.exe not found; install the Windows SDK (winget install Microsoft.WindowsSDK)'
}
$signtool = Find-SdkTool 'signtool.exe'

$stagingRoot = Join-Path $projectDir "target\$Profile\msix-staging"
$staging = Join-Path $stagingRoot 'package'
if (Test-Path $stagingRoot) { Remove-Item -Recurse -Force $stagingRoot }
New-Item -ItemType Directory -Path (Join-Path $staging 'assets') -Force | Out-Null

Copy-Item $exePath $staging
Copy-Item (Join-Path $projectDir 'assets\brand\resolved-icon.png') `
    (Join-Path $staging 'assets\resolved-icon.png')

# ProcessorArchitecture: Resolved currently ships x64; the exe's PE header is
# not parsed because MakeAppx validates architecture separately. Keep the
# manifest arch aligned with the build; extend here when arm64 is supported.
$peArch = 'x64'

# AppxManifest: the Identity Publisher must equal the signing certificate's
# subject, or Appx installation rejects the package (0x800B0100).
$manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities">
  <Identity Name="$identityName"
            Version="$manifestVersion"
            Publisher="$certificateSubject"
            ProcessorArchitecture="$peArch" />
  <Properties>
    <DisplayName>Resolved</DisplayName>
    <PublisherDisplayName>Nous Research</PublisherDisplayName>
    <Logo>assets\resolved-icon.png</Logo>
  </Properties>
  <Resources>
    <Resource Language="en-us" />
  </Resources>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0" />
  </Dependencies>
  <Applications>
    <Application Id="Resolved"
                 Executable="api-tester.exe"
                 EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements
        DisplayName="Resolved"
        Description="A native API workbench that keeps request intent, wire state, and responses explicit"
        BackgroundColor="transparent"
        Square150x150Logo="assets\resolved-icon.png"
        Square44x44Logo="assets\resolved-icon.png" />
    </Application>
  </Applications>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>
</Package>
"@
Set-Content -Path (Join-Path $staging 'AppxManifest.xml') -Value $manifest -Encoding utf8

$msixPath = Join-Path $projectDir "target\$Profile\Resolved.msix"
Write-Host "Packing $msixPath ..."
& $makeAppx.FullName pack /o /d $staging /p $msixPath
if ($LASTEXITCODE -ne 0) { throw "MakeAppx pack failed with $LASTEXITCODE" }

if ($signtool) {
    $cert = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $certificateSubject -and $_.HasPrivateKey } |
        Sort-Object NotBefore -Descending | Select-Object -First 1
    if ($cert) {
        Write-Host "Signing with $($cert.Thumbprint) ..."
        & $signtool.FullName sign /fd SHA256 /sha1 $cert.Thumbprint $msixPath | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "signtool sign failed with $LASTEXITCODE" }
    } else {
        Write-Warning 'No matching signing certificate; the MSIX is packed but unsigned.'
        Write-Warning "Create one with:  powershell -File scripts/package-msix.ps1 -InstallCert"
        return
    }
} else {
    Write-Warning 'signtool.exe not found; the MSIX is packed but unsigned.'
    return
}

Remove-Item -Recurse -Force $stagingRoot
Write-Host "Packaged $msixPath"

if ($Install) {
    # Make the certificate trusted for package validation. Add-AppxPackage
    # refuses an untrusted signature; trusting CurrentUser\Root + Trust is
    # sufficient for a per-user install (no admin needed).
    $trustedRoot = Get-ChildItem Cert:\CurrentUser\Root |
        Where-Object { $_.Thumbprint -eq $cert.Thumbprint }
    if (-not $trustedRoot) {
        Write-Host 'Trusting the signing certificate for the current user...'
        $store = New-Object System.Security.Cryptography.X509Certificates.X509Store('Root', 'CurrentUser')
        $store.Open('ReadWrite')
        $store.Add($cert)
        $store.Close()
    }
    Write-Host 'Installing the package...'
    Add-AppxPackage -Path $msixPath
    Write-Host 'Installed. Launch with:'
    Write-Host '  Start-Process "shell:AppsFolder\nous.resolved_rst4z8a1w!Resolved"'
    Write-Host 'or from the Start menu ("Resolved").'
} else {
    Write-Host 'Install it with:'
    Write-Host "  powershell -File scripts\package-msix.ps1 -Install"
    Write-Host 'or: Add-AppxPackage -Path <Resolved.msix>'
    Write-Host 'Launch from the Start menu ("Resolved").'
}
