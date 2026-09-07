# Windows dependency preparation, mirroring scripts/prepare-gpui.sh.
#
# Downloads the pinned crates.io archives when they are not already in the
# cargo cache, verifies their SHA-256, and applies the repo's patches with
# cumulative-hash marker files so later runs can skip re-patching.
#
# Usage: powershell -File scripts/prepare-gpui.ps1

$ErrorActionPreference = 'Stop'

$projectDir = Split-Path -Parent $PSScriptRoot
$vendorRoot = Join-Path $projectDir 'vendor'

$pinned = @(
    @{
        name    = 'gpui'
        version = '0.2.2'
        sha256  = '979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707'
        patches = @(
            "$projectDir\patches\gpui-0.2.2-metal-memoryless.patch",
            "$projectDir\patches\gpui-0.2.2-retained-line-layout-cache.patch",
            "$projectDir\patches\gpui-0.2.2-reentrant-async-context.patch",
            "$projectDir\patches\gpui-0.2.2-windows-clip-children.patch",
            "$projectDir\patches\gpui-0.2.2-linux-raw-window-handle.patch",
            "$projectDir\patches\gpui-0.2.2-configurable-tab-width.patch"
        )
    },
    @{
        name    = 'gpui-component'
        version = '0.5.1'
        sha256  = 'd021d46b4088d3d93a57ccdf443da85695a77272108caca2f6fe5369f584966a'
        patches = @(
            "$projectDir\patches\gpui-component-0.5.1-input-integration.patch",
            "$projectDir\patches\gpui-component-0.5.1-code-folding.patch",
            "$projectDir\patches\gpui-component-0.5.1-indent-guide-layout.patch",
            "$projectDir\patches\gpui-component-0.5.1-responsive-settings-sidebar.patch",
            "$projectDir\patches\gpui-component-0.5.1-inline-actions.patch",
            "$projectDir\patches\gpui-component-0.5.1-public-input-menu-state.patch",
            "$projectDir\patches\gpui-component-0.5.1-completion-edge-placement.patch",
            "$projectDir\patches\gpui-component-0.5.1-configurable-active-line.patch",
            "$projectDir\patches\gpui-component-0.5.1-switch-contrast.patch"
        )
    }
)

function Get-FileSha256([string]$path) {
    (Get-FileHash -Path $path -Algorithm SHA256).Hash.ToLower()
}

function Get-PatchedTreeSha256([string]$root) {
    $lines = Get-ChildItem -Path $root -File -Recurse |
        Where-Object { $_.Name -ne '.api-tester-patch-sha256' } |
        ForEach-Object {
            $relative = [IO.Path]::GetRelativePath($root, $_.FullName).Replace('\', '/')
            "$(Get-FileSha256 $_.FullName)  ./$relative"
        } |
        Sort-Object
    $canonical = ($lines -join "`n") + "`n"
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes($canonical)
    $hasher = [Security.Cryptography.SHA256]::Create()
    try {
        -join ($hasher.ComputeHash($bytes) | ForEach-Object { $_.ToString('x2') })
    } finally {
        $hasher.Dispose()
    }
}

function Invoke-PrepareCrate($crate) {
    $crateArchive = "$($crate.name)-$($crate.version).crate"
    $vendorDir = Join-Path $vendorRoot "$($crate.name)-$($crate.version)"
    $markerFile = Join-Path $vendorDir '.api-tester-patch-sha256'

    # Match prepare-gpui.sh exactly so either platform can reuse a vendor tree
    # prepared by the other one.
    $patchSha = ($crate.patches |
        ForEach-Object { Get-FileSha256 $_ }) -join ':'

    $sourceIsCurrent =
        (Test-Path $markerFile) -and
        ((Get-Content $markerFile -TotalCount 1) -eq $patchSha) -and
        ((Get-Content $markerFile | Select-Object -Skip 1 -First 1) -eq $crate.sha256) -and
        ((Get-Content $markerFile | Select-Object -Skip 2 -First 1) -eq
            (Get-PatchedTreeSha256 $vendorDir))
    if ($sourceIsCurrent) {
        return
    }

    $tempDir = Join-Path ([IO.Path]::GetTempPath()) ("api-tester-$($crate.name)." + [IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $tempDir | Out-Null
    try {
        $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
        $archive = $null
        $cacheDirs = Get-ChildItem (Join-Path $cargoHome 'registry/cache') -Directory -ErrorAction SilentlyContinue
        foreach ($cacheDir in $cacheDirs) {
            $candidate = Join-Path $cacheDir.FullName $crateArchive
            if (Test-Path $candidate) {
                $archive = $candidate
                break
            }
        }
        if (-not $archive) {
            $archive = Join-Path $tempDir $crateArchive
            Write-Host "Downloading $($crate.name) $($crate.version) from crates.io..."
            $url = "https://static.crates.io/crates/$($crate.name)/$crateArchive"
            Invoke-WebRequest -Uri $url -OutFile $archive -UserAgent 'resolved-setup/1.0'
        }

        $actual = Get-FileSha256 $archive
        if ($actual -ne $crate.sha256) {
            throw "checksum mismatch for $crateArchive (expected $($crate.sha256), got $actual)"
        }

        $extractDir = Join-Path $tempDir 'extract'
        New-Item -ItemType Directory -Path $extractDir | Out-Null
        tar -xzf $archive -C $extractDir
        if ($LASTEXITCODE -ne 0) { throw "tar extraction failed for $crateArchive" }
        $sourceDir = Join-Path $extractDir "$($crate.name)-$($crate.version)"
        if (-not (Test-Path $sourceDir)) {
            throw "unexpected archive layout for $crateArchive"
        }

        foreach ($patch in $crate.patches) {
            Push-Location $sourceDir
            try {
                # `git apply` works outside a repository and is available with
                # the Git installation already required by this source tree.
                # Normalize checkout CRLF before applying to the LF crate.
                $normalizedPatch = Join-Path $tempDir ([IO.Path]::GetRandomFileName())
                $patchText = [IO.File]::ReadAllText($patch).Replace("`r`n", "`n")
                [IO.File]::WriteAllText(
                    $normalizedPatch,
                    $patchText,
                    [Text.UTF8Encoding]::new($false)
                )
                & git apply --recount --unidiff-zero --whitespace=nowarn --unsafe-paths $normalizedPatch
                if ($LASTEXITCODE -ne 0) {
                    throw "patch no longer applies: $patch"
                }
            } finally {
                Pop-Location
            }
        }

        $treeSha = Get-PatchedTreeSha256 $sourceDir
        Set-Content -Path (Join-Path $sourceDir '.api-tester-patch-sha256') `
            -Value @($patchSha, $crate.sha256, $treeSha) -Encoding ascii

        New-Item -ItemType Directory -Path $vendorRoot -Force | Out-Null
        if (Test-Path $vendorDir) { Remove-Item -Recurse -Force $vendorDir }
        Move-Item $sourceDir $vendorDir
        Write-Host "Prepared patched $($crate.name) $($crate.version) in $vendorDir"
    } finally {
        Remove-Item -Recurse -Force $tempDir -ErrorAction SilentlyContinue
    }
}

foreach ($crate in $pinned) {
    Invoke-PrepareCrate $crate
}
