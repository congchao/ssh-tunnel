$ErrorActionPreference = "Stop"

$RootDir = Resolve-Path (Join-Path $PSScriptRoot "..")
$Target = "x86_64-pc-windows-msvc"
$DefaultTargetDir = "D:\ssh-tunnel-target"
$NsisVersion = "3.11"
$NsisZipName = "nsis-$NsisVersion.zip"
$NsisUrl = "https://github.com/tauri-apps/binary-releases/releases/download/nsis-$NsisVersion/$NsisZipName"
$NsisTauriUtilsUrl = "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll"
$ManualDownloadDir = Join-Path $PSScriptRoot "downloads"

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

function Invoke-DownloadFile {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Url,
        [Parameter(Mandatory = $true)]
        [string] $OutFile
    )

    New-Item -ItemType Directory -Force -Path (Split-Path $OutFile -Parent) | Out-Null
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $Url -OutFile $OutFile -TimeoutSec 600
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

function Add-EnvFlag {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Name,
        [Parameter(Mandatory = $true)]
        [string] $Flag
    )

    $CurrentValue = [System.Environment]::GetEnvironmentVariable($Name, "Process")
    if ([string]::IsNullOrWhiteSpace($CurrentValue)) {
        [System.Environment]::SetEnvironmentVariable($Name, $Flag, "Process")
    } elseif ($CurrentValue -notlike "*$Flag*") {
        [System.Environment]::SetEnvironmentVariable($Name, "$CurrentValue $Flag", "Process")
    }
}

function Test-TauriNsisCache {
    param(
        [Parameter(Mandatory = $true)]
        [string] $NsisDir
    )

    $RequiredFiles = @(
        "makensis.exe",
        "Bin\makensis.exe",
        "Stubs\lzma-x86-unicode",
        "Stubs\lzma_solid-x86-unicode",
        "Plugins\x86-unicode\additional\nsis_tauri_utils.dll",
        "Include\MUI2.nsh",
        "Include\FileFunc.nsh",
        "Include\x64.nsh",
        "Include\nsDialogs.nsh",
        "Include\WinMessages.nsh",
        "Include\Win\COM.nsh",
        "Include\Win\Propkey.nsh",
        "Include\Win\RestartManager.nsh"
    )

    foreach ($RelativePath in $RequiredFiles) {
        if (-not (Test-Path (Join-Path $NsisDir $RelativePath))) {
            return $false
        }
    }

    return $true
}

function Initialize-TauriNsisCache {
    $TauriToolsDir = Join-Path $env:LOCALAPPDATA "tauri"
    $NsisDir = Join-Path $TauriToolsDir "NSIS"

    if (Test-TauriNsisCache -NsisDir $NsisDir) {
        Write-Host "Tauri NSIS cache is ready: $NsisDir"
        return
    }

    Write-Host "Preparing Tauri NSIS cache: $NsisDir"
    Remove-Item -Recurse -Force $NsisDir -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $TauriToolsDir | Out-Null

    $LocalNsisZip = Join-Path $ManualDownloadDir $NsisZipName
    if (-not (Test-Path $LocalNsisZip)) {
        Write-Host "Downloading NSIS package: $NsisUrl"
        try {
            Invoke-DownloadFile -Url $NsisUrl -OutFile $LocalNsisZip
        } catch {
            Write-Error "Failed to download NSIS package. Download it manually from $NsisUrl and save it as $LocalNsisZip, then rerun yarn.cmd package:win:amd64"
        }
    } else {
        Write-Host "Using local NSIS package: $LocalNsisZip"
    }

    $ExtractDir = Join-Path $TauriToolsDir "nsis-$NsisVersion"
    Remove-Item -Recurse -Force $ExtractDir -ErrorAction SilentlyContinue
    Expand-Archive -Path $LocalNsisZip -DestinationPath $TauriToolsDir -Force
    Move-Item -Force $ExtractDir $NsisDir

    $PluginDir = Join-Path $NsisDir "Plugins\x86-unicode\additional"
    New-Item -ItemType Directory -Force -Path $PluginDir | Out-Null

    $LocalNsisTauriUtils = Join-Path $ManualDownloadDir "nsis_tauri_utils.dll"
    if (-not (Test-Path $LocalNsisTauriUtils)) {
        Write-Host "Downloading Tauri NSIS helper: $NsisTauriUtilsUrl"
        try {
            Invoke-DownloadFile -Url $NsisTauriUtilsUrl -OutFile $LocalNsisTauriUtils
        } catch {
            Write-Error "Failed to download nsis_tauri_utils.dll. Download it manually from $NsisTauriUtilsUrl and save it as $LocalNsisTauriUtils, then rerun yarn.cmd package:win:amd64"
        }
    } else {
        Write-Host "Using local Tauri NSIS helper: $LocalNsisTauriUtils"
    }

    Copy-Item -Force $LocalNsisTauriUtils (Join-Path $PluginDir "nsis_tauri_utils.dll")

    if (-not (Test-TauriNsisCache -NsisDir $NsisDir)) {
        Write-Error "Tauri NSIS cache is still incomplete after preparation: $NsisDir"
    }
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

if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    Write-Error "cmake was not found. Install CMake and make sure it is available in PATH. For example: winget install Kitware.CMake"
}

if (-not (Get-Command nasm -ErrorAction SilentlyContinue)) {
    Write-Error "nasm was not found. Install NASM and make sure it is available in PATH. For example: winget install NASM.NASM"
}

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

Write-Host "Configuring aws-lc-sys C standard flags"
$env:AWS_LC_SYS_C_STD = "11"
Add-EnvFlag -Name "CFLAGS" -Flag "/std:c11"
Add-EnvFlag -Name "CFLAGS_x86_64_pc_windows_msvc" -Flag "/std:c11"

Write-Host "Installing Rust target: $Target"
Invoke-Checked rustup target add $Target

Write-Host "Installing frontend dependencies"
Invoke-Checked yarn install --frozen-lockfile

Initialize-TauriNsisCache

Write-Host "Building Windows amd64 package"
Invoke-Checked yarn tauri build --target $Target

Write-Host "Done. Artifacts:"
Write-Host "  $env:CARGO_TARGET_DIR\$Target\release\bundle"
