# Windows dependency preparation, mirroring scripts/prepare-typescript-service.sh.
#
# Downloads the pinned TypeScript 6.0.2 language-service archive from npm,
# verifies its SHA-256, extracts only the compiler + standard-library assets
# Resolved vendors, and rewrites those files with CRLF-safe content untouched.
#
# Usage: powershell -File scripts/prepare-typescript-service.ps1

$ErrorActionPreference = 'Stop'

$projectDir = Split-Path -Parent $PSScriptRoot
$typescriptVersion = '6.0.2'
$archiveName = "typescript-$typescriptVersion.tgz"
$archiveUrl = "https://registry.npmjs.org/typescript/-/$archiveName"
$archiveSha256 = '0ae5c188a2f5db22df72fe5e74dcbc122afb52031a86dbac33e78a86db39c65e'
$vendorRoot = Join-Path $projectDir 'vendor'
$vendorDir = Join-Path $vendorRoot "typescript-service-$typescriptVersion"
$markerFile = Join-Path $vendorDir '.source-sha256'
$expectedDeclarationCount = 99

function Get-FileSha256([string]$path) {
    (Get-FileHash -Path $path -Algorithm SHA256).Hash.ToLower()
}

$libDir = Join-Path $vendorDir 'lib'
$required = @(
    (Join-Path $libDir 'typescript.js'),
    (Join-Path $libDir 'lib.es5.d.ts'),
    (Join-Path $libDir 'lib.esnext.d.ts'),
    (Join-Path $libDir 'lib.decorators.d.ts'),
    (Join-Path $libDir 'lib.decorators.legacy.d.ts'),
    (Join-Path $vendorDir 'LICENSE.txt'),
    (Join-Path $vendorDir 'ThirdPartyNoticeText.txt')
)
$declarationCount = @(Get-ChildItem -Path $libDir -Filter '*.d.ts' -ErrorAction SilentlyContinue).Count
$preparedIsCurrent =
    (Test-Path $markerFile) -and
    ((Get-Content $markerFile -TotalCount 1) -eq $archiveSha256) -and
    (($required | Where-Object { -not (Test-Path $_) }).Count -eq 0) -and
    ($declarationCount -eq $expectedDeclarationCount)
if ($preparedIsCurrent) {
    exit 0
}

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("resolved-typescript-service." + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $archive = Join-Path $tempRoot $archiveName
    Write-Host "Downloading TypeScript $typescriptVersion language service from npm..."
    Invoke-WebRequest -Uri $archiveUrl -OutFile $archive -UserAgent 'resolved-setup/1.0'

    $actual = Get-FileSha256 $archive
    if ($actual -ne $archiveSha256) {
        throw "checksum mismatch for $archiveName (expected $archiveSha256, got $actual)"
    }

    $extracted = Join-Path $tempRoot 'extracted'
    $pkgLib = Join-Path $extracted 'package/lib'
    New-Item -ItemType Directory -Path (Join-Path $extracted 'package') | Out-Null
    tar -xzf $archive -C $extracted package/lib/typescript.js `
        'package/lib/lib.es5.d.ts' 'package/lib/lib.esnext.d.ts' `
        'package/lib/lib.decorators.d.ts' 'package/lib/lib.decorators.legacy.d.ts' `
        package/LICENSE.txt package/ThirdPartyNoticeText.txt
    if ($LASTEXITCODE -ne 0) { throw "tar extraction failed for $archiveName" }

    # Validate the curated member set: exactly 99 .d.ts files plus the
    # compiler, license, and notices, mirroring the shell driver's checks.
    $declarations = @(Get-ChildItem -Path $pkgLib -Filter 'lib.es*.d.ts')
    $decorators = @(Get-ChildItem -Path $pkgLib -Filter 'lib.decorators*.d.ts')
    if (($declarations.Count + 1) -ne $expectedDeclarationCount) {
        throw "TypeScript $typescriptVersion archive layout changed: found $($declarations.Count + 1) declaration files, expected $expectedDeclarationCount"
    }
    if ($decorators.Count -lt 2) {
        throw "TypeScript $typescriptVersion archive layout changed: lib.decorators* missing"
    }

    New-Item -ItemType Directory -Path (Join-Path $vendorDir 'lib') -Force | Out-Null
    Copy-Item (Join-Path $pkgLib 'typescript.js') (Join-Path $vendorDir 'lib/typescript.js')
    foreach ($declaration in $declarations) {
        Copy-Item $declaration.FullName (Join-Path $vendorDir "lib/$($declaration.Name)")
    }
    Copy-Item (Join-Path $extracted 'package/LICENSE.txt') (Join-Path $vendorDir 'LICENSE.txt')
    Copy-Item (Join-Path $extracted 'package/ThirdPartyNoticeText.txt') `
        (Join-Path $vendorDir 'ThirdPartyNoticeText.txt')
    Set-Content -Path $markerFile -Value $archiveSha256 -Encoding ascii

    Write-Host "Prepared TypeScript $typescriptVersion language service in $vendorDir"
} finally {
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
}
