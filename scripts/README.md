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

Run on Windows PowerShell 5.1 or PowerShell 7 with Rust, Node.js, Yarn, and Microsoft C++ Build Tools installed:

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

Make sure Visual Studio Build Tools includes:

- Desktop development with C++
- MSVC v143 or newer
- Windows 10/11 SDK

If `cl.exe` is not found, install Visual Studio Build Tools or run the package command from `x64 Native Tools Command Prompt for VS`.

If MSBuild reports `DirectoryNotFoundException` for a deep build path, it is usually a Windows path length issue. The Windows package script sets `CARGO_TARGET_DIR=D:\ssh-tunnel-target` by default to keep build paths short. You can also extract the project to a shorter directory such as `D:\src\ssh-tunnel`.
