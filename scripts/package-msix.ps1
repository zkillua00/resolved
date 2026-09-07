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
    [switch]$InstallCert,
    [ValidateRange(0, 65535)]
    [int]$Revision = 0
)

$ErrorActionPreference = 'Stop'

$projectDir = Split-Path -Parent $PSScriptRoot
$exePath = Join-Path $projectDir "target\$Profile\api-tester.exe"
if (-not (Test-Path $exePath)) {
    throw "missing $exePath - run 'scripts/cargo.ps1 build' first"
}

$identityName = 'nous.resolved'
$certificateSubject = 'CN=Nous Research'
$publisher = $certificateSubject.Substring(3)
$version = (& cargo metadata --no-deps --format-version 1 |
    ConvertFrom-Json).packages[0].version
# MSIX versions use four numeric parts. Release packages start at revision 0;
# repeated local installs of the same Cargo version advance the revision so
# Windows deploys the rebuilt executable instead of retaining stale bytes.
$manifestVersion = [version]"$version.$Revision"
if ($Install) {
    $installed = Get-AppxPackage -Name $identityName -ErrorAction SilentlyContinue |
        Sort-Object Version -Descending |
        Select-Object -First 1
    if ($installed) {
        $installedVersion = [version]$installed.Version
        $sameBaseVersion = $installedVersion.Major -eq $manifestVersion.Major -and
            $installedVersion.Minor -eq $manifestVersion.Minor -and
            $installedVersion.Build -eq $manifestVersion.Build
        if ($sameBaseVersion -and $installedVersion.Revision -ge $manifestVersion.Revision) {
            if ($installedVersion.Revision -ge 65535) {
                throw "MSIX revision limit reached for $version; bump the Cargo package version"
            }
            $manifestVersion = [version]::new(
                $manifestVersion.Major,
                $manifestVersion.Minor,
                $manifestVersion.Build,
                $installedVersion.Revision + 1
            )
        }
    }
}
$manifestVersion = $manifestVersion.ToString()

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
$sourceLogoPath = Join-Path $projectDir 'assets\brand\resolved-icon.png'
$stagedLogoPath = Join-Path $staging 'assets\resolved-icon.png'
Copy-Item $sourceLogoPath $stagedLogoPath

# Packaged desktop apps use Square44x44Logo target-size resources for the
# taskbar, Alt+Tab, Task View, and shell menus. Without unplated variants,
# Windows puts the transparent logo on an accent-colored backplate. Generate
# every documented target size from the full-resolution source and provide
# both dark- and light-shell unplated alternatives so the rounded transparent
# corners survive on either taskbar theme.
Add-Type -AssemblyName System.Drawing
$sourceLogo = [System.Drawing.Bitmap]::FromFile($sourceLogoPath)
$targetSizes = @(16, 20, 24, 30, 32, 36, 40, 44, 48, 60, 64, 72, 80, 96, 256)
try {
    foreach ($targetSize in $targetSizes) {
        $targetLogo = [System.Drawing.Bitmap]::new(
            $targetSize,
            $targetSize,
            [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
        )
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($targetLogo)
            try {
                $graphics.Clear([System.Drawing.Color]::Transparent)
                $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
                $graphics.CompositingQuality = `
                    [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
                $graphics.InterpolationMode = `
                    [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                $graphics.DrawImage(
                    $sourceLogo,
                    [System.Drawing.Rectangle]::new(0, 0, $targetSize, $targetSize)
                )
            } finally {
                $graphics.Dispose()
            }

            foreach ($alternateForm in @('', '_altform-unplated', '_altform-lightunplated')) {
                $targetName = "resolved-icon.targetsize-$targetSize$alternateForm.png"
                $targetLogo.Save(
                    (Join-Path $staging "assets\$targetName"),
                    [System.Drawing.Imaging.ImageFormat]::Png
                )
            }
        } finally {
            $targetLogo.Dispose()
        }
    }
} finally {
    $sourceLogo.Dispose()
}

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

# Resource qualifiers such as targetsize and altform are selected through the
# package resource index. MakeAppx does not create it automatically.
$makePri = Find-SdkTool 'makepri.exe'
if (-not $makePri) {
    throw 'MakePri.exe not found; install the Windows SDK'
}
$priConfig = Join-Path $stagingRoot 'priconfig.xml'
& $makePri.FullName createconfig /cf $priConfig /dq en-US /o | Out-Null
if ($LASTEXITCODE -ne 0) { throw "MakePri createconfig failed with $LASTEXITCODE" }
& $makePri.FullName new /pr $staging /cf $priConfig `
    /of (Join-Path $staging 'resources.pri') /o | Out-Null
if ($LASTEXITCODE -ne 0) { throw "MakePri new failed with $LASTEXITCODE" }

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
    Add-AppxPackage -Path $msixPath `
        -ForceApplicationShutdown `
        -ForceUpdateFromAnyVersion
    $installedPackage = Get-AppxPackage -Name $identityName | Select-Object -First 1
    $appUserModelId = "$($installedPackage.PackageFamilyName)!Resolved"
    Write-Host 'Installed. Launch with:'
    Write-Host "  Start-Process 'shell:AppsFolder\$appUserModelId'"
    Write-Host 'or from the Start menu ("Resolved").'
} else {
    Write-Host 'Install it with:'
    Write-Host "  powershell -File scripts\package-msix.ps1 -Install"
    Write-Host 'or: Add-AppxPackage -Path <Resolved.msix>'
    Write-Host 'Launch from the Start menu ("Resolved").'
}
