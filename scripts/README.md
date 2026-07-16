# Build Scripts

## macOS Apple Silicon

Run on macOS:

```bash
yarn package:mac:arm64
```

Target:

```text
aarch64-apple-darwin
```

Artifacts:

```text
src-tauri/target/aarch64-apple-darwin/release/bundle
```

## macOS Intel

Run on macOS:

```bash
yarn package:mac:x64
```

Target:

```text
x86_64-apple-darwin
```

Artifacts:

```text
src-tauri/target/x86_64-apple-darwin/release/bundle
```

## Windows amd64

Run on Windows PowerShell 5.1 or PowerShell 7 with Rust, Node.js, Yarn, CMake, NASM, and Microsoft C++ Build Tools installed:

```powershell
yarn package:win:amd64
```

Target:

```text
x86_64-pc-windows-msvc
```

Artifacts:

```text
D:\ssh-tunnel-target\x86_64-pc-windows-msvc\release\bundle
```

Windows packages should be built on Windows or a Windows CI runner. Cross-building Windows Tauri bundles from macOS is not recommended because the WebView2/MSVC bundling toolchain is Windows-native.

If the build fails with `Missing dependency: cmake`, install CMake and reopen the terminal:

```powershell
winget install Kitware.CMake
```

If the build fails with `NASM command not found or failed to execute`, install NASM and reopen the terminal:

```powershell
winget install NASM.NASM
```

If the build fails while downloading `nsis-3.11.zip`, download these files manually:

```text
https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip
https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll
```

Save them as:

```text
scripts\downloads\nsis-3.11.zip
scripts\downloads\nsis_tauri_utils.dll
```

Then clear the incomplete Tauri NSIS cache and retry:

```powershell
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\tauri\NSIS" -ErrorAction SilentlyContinue
yarn.cmd package:win:amd64
```

Make sure Visual Studio Build Tools includes:

- Desktop development with C++
- MSVC v143 or newer
- Windows 10/11 SDK
- C++ CMake tools for Windows

If `cl.exe` is not found, install Visual Studio Build Tools or run the package command from `x64 Native Tools Command Prompt for VS`.

If the build fails with `C atomics require C11 or later`, make sure you are using the latest Windows build script. It sets:

```powershell
AWS_LC_SYS_C_STD=11
CFLAGS=/std:c11
```

After changing compiler flags, clean the previous Rust target cache before retrying:

```powershell
Remove-Item -Recurse -Force D:\ssh-tunnel-target
yarn.cmd package:win:amd64
```

If MSBuild reports `DirectoryNotFoundException` for a deep build path, it is usually a Windows path length issue. The Windows package script sets `CARGO_TARGET_DIR=D:\ssh-tunnel-target` by default to keep build paths short. You can also extract the project to a shorter directory such as `D:\src\ssh-tunnel`.
