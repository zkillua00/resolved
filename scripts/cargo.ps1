# Resolved build driver for Windows, mirroring scripts/cargo.sh.
#
# Usage:
#   powershell -File scripts/cargo.ps1                      # cargo check
#   powershell -File scripts/cargo.ps1 build                 # debug build
#   powershell -File scripts/cargo.ps1 run -- [--release]    # package as MSIX and launch
#   powershell -File scripts/cargo.ps1 test                  # cargo test
#   powershell -File scripts/cargo.ps1 -- <any cargo args>   # pass-through
#
# Prerequisites: Visual Studio Build Tools (C++ workload), Rust MSVC,
# and the MSIX Packaging toolchain (MakeAppx.exe from the Windows SDK,
# or 'winget install Microsoft.WindowsSDK.10.0.26100' for the SDK tools).
# Cargo uses every logical processor by default. Override that only when a
# machine needs a lower cap, for example: $env:CARGO_BUILD_JOBS = 16

$ErrorActionPreference = 'Stop'

$projectDir = Split-Path -Parent $PSScriptRoot
Set-Location $projectDir

function Initialize-WindowsBuildToolchain {
    # Keep an explicit caller-provided linker. Otherwise use the copy of lld
    # bundled with Rust: it is faster and more parallel than link.exe during
    # the final link, without adding another machine prerequisite.
    $linkerVariable = 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER'
    if ([Environment]::GetEnvironmentVariable($linkerVariable, 'Process')) {
        return
    }

    $rustSysroot = (& rustc --print sysroot).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'failed to locate the Rust toolchain' }

    $rustLld = Join-Path $rustSysroot 'lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe'
    if (Test-Path -LiteralPath $rustLld) {
        [Environment]::SetEnvironmentVariable($linkerVariable, $rustLld, 'Process')
    }
}

Initialize-WindowsBuildToolchain

function Invoke-Prepare {
    & "$projectDir\scripts\prepare-gpui.ps1"
    & "$projectDir\scripts\prepare-typescript-service.ps1"
    if ($LASTEXITCODE -ne 0) { throw "dependency preparation failed" }
}

if ($args.Count -eq 0) {
    Invoke-Prepare
    & cargo check @args
    exit $LASTEXITCODE
}

switch ($args[0]) {
    'check' {
        Invoke-Prepare
        & cargo check @($args | Select-Object -Skip 1)
        exit $LASTEXITCODE
    }
    'build' {
        Invoke-Prepare
        & cargo build @($args | Select-Object -Skip 1)
        exit $LASTEXITCODE
    }
    'test' {
        Invoke-Prepare
        & cargo test @($args | Select-Object -Skip 1)
        exit $LASTEXITCODE
    }
    'run' {
        # Mirrors cargo.sh run: build, package into an MSIX shell that
        # supplies package identity, then launch with the optional args.
        $profile = 'debug'
        $rest = @($args | Select-Object -Skip 1)
        if ($rest.Count -gt 0 -and $rest[0] -eq '--release') {
            $profile = 'release'
            $rest = @($rest | Select-Object -Skip 1)
        }
        & cargo build --profile $profile
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        & "$projectDir\scripts\package-msix.ps1" -Profile $profile
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        $exe = Join-Path $projectDir "target\$profile\api-tester.exe"
        if ($rest.Count -gt 0) {
            & $exe @rest
        } else {
            Start-Process -FilePath $exe
        }
        exit $LASTEXITCODE
    }
    '--' {
        Invoke-Prepare
        & cargo @($args | Select-Object -Skip 1)
        exit $LASTEXITCODE
    }
    default {
        # Unrecognized first argument: treat everything as cargo passthrough,
        # matching scripts/cargo.sh's exec cargo "$@" behavior.
        Invoke-Prepare
        & cargo @args
        exit $LASTEXITCODE
    }
}
