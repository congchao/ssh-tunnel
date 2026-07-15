$ErrorActionPreference = "Stop"

$RootDir = Resolve-Path (Join-Path $PSScriptRoot "..")
$Target = "x86_64-pc-windows-msvc"
$DefaultTargetDir = "D:\ssh-tunnel-target"

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Command,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]] $Arguments
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Command failed with exit code ${LASTEXITCODE}: $Command $($Arguments -join ' ')"
    }
}

function Import-VsDevShell {
    $VsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $VsWhere)) {
        return $false
    }

    $VsInstallPath = & $VsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $VsInstallPath) {
        return $false
    }

    $VsDevCmd = Join-Path $VsInstallPath "Common7\Tools\VsDevCmd.bat"
    if (-not (Test-Path $VsDevCmd)) {
        return $false
    }

    Write-Host "Loading Visual Studio build environment"
    cmd /c "`"$VsDevCmd`" -arch=x64 -host_arch=x64 > nul && set" | ForEach-Object {
        if ($_ -match "^(.*?)=(.*)$") {
            [System.Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
        }
    }

    return $true
}

$IsWindowsHost = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)

if (-not $IsWindowsHost) {
    Write-Error "This script must run on Windows with the MSVC build tools installed."
}

Set-Location $RootDir

if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    $env:CARGO_TARGET_DIR = $DefaultTargetDir
}
New-Item -ItemType Directory -Force -Path $env:CARGO_TARGET_DIR | Out-Null
Write-Host "Using Cargo target dir: $env:CARGO_TARGET_DIR"

Write-Host "Clearing Rust environment variables that can pollute dependency builds"
Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
Remove-Item Env:\CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
Remove-Item Env:\RUSTC_WRAPPER -ErrorAction SilentlyContinue
Remove-Item Env:\RUSTC_WORKSPACE_WRAPPER -ErrorAction SilentlyContinue

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    Write-Error "rustup was not found. Install Rust first: https://www.rust-lang.org/tools/install"
}

if (-not (Get-Command yarn -ErrorAction SilentlyContinue)) {
    Write-Error "yarn was not found. Install Node.js and Yarn first."
}

Write-Host "Rust toolchain:"
Invoke-Checked rustc -vV
Invoke-Checked cargo -vV

if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    $LoadedVsEnv = Import-VsDevShell
    if (-not $LoadedVsEnv -or -not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        Write-Error "MSVC compiler cl.exe was not found. Install Visual Studio Build Tools with the 'Desktop development with C++' workload and Windows SDK, then reopen PowerShell."
    }
}

Write-Host "MSVC compiler:"
& cl.exe /Bv
$LastExitCode = 0

$CompilerProbeDir = Join-Path $env:TEMP "ssh-tunnel-msvc-probe"
$CompilerProbeFile = Join-Path $CompilerProbeDir "probe.c"
New-Item -ItemType Directory -Force -Path $CompilerProbeDir | Out-Null
Set-Content -Path $CompilerProbeFile -Value "int main(void) { return 0; }" -Encoding ASCII
Push-Location $CompilerProbeDir
try {
    Invoke-Checked cl.exe /nologo probe.c
} finally {
    Pop-Location
}

Write-Host "Installing Rust target: $Target"
Invoke-Checked rustup target add $Target

Write-Host "Installing frontend dependencies"
Invoke-Checked yarn install --frozen-lockfile

Write-Host "Building Windows amd64 package"
Invoke-Checked yarn tauri build --target $Target

Write-Host "Done. Artifacts:"
Write-Host "  $env:CARGO_TARGET_DIR\$Target\release\bundle"
