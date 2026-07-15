# SSH 隧道管理器

一个基于 Tauri + Vue 3 的轻量级 SSH 隧道管理工具，支持多端口映射、实时日志、托盘常驻与中英文界面切换。

## 功能亮点

- 多端口 SSH 隧道映射（本地监听 → 远程目标）
- 实时日志展示
- 托盘常驻（后台运行 + 右键菜单）
- 中英文界面切换（默认跟随系统语言）

## 运行步骤

1. 安装依赖

```bash
yarn
```

2. 启动桌面应用（Tauri）

```bash
yarn tauri dev
```

> 如仅需启动前端调试：`yarn dev`

## Release 打包

一键构建 macOS x64、macOS arm64、Windows x64 release 包：

```bash
yarn release
```

构建 macOS x64 release 包：

```bash
yarn package:mac:x64
```

构建 macOS arm64 release 包：

```bash
yarn package:mac:arm64
```

构建 Windows x64 release 包：

```bash
yarn package:win:amd64
```

如缺少 Rust target，可先安装：

```bash
rustup target add x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc
```

> Windows x64 包通常需要在 Windows 环境或配置好对应交叉编译/打包工具链的环境中构建。
> 旧命令 `yarn release:mac:x64`、`yarn release:mac:arm64`、`yarn release:windows:x64` 仍可使用。

## 界面截图

### 运行界面

![运行界面](docs/running.png)

### 配置界面

![配置界面](docs/setting.png)
