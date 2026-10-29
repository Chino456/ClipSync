# ClipSync

简体中文 | [English](README.md)

ClipSync 是一个逐步开发的跨平台剪贴板同步工具，用于在同一局域网内的
Windows 和 Linux 设备之间同步剪贴板。

## 当前能力

- UDP 组播自动发现局域网设备。
- 在禁用组播的校园网中通过 IPv4 地址直接发现设备。
- 通过一条双向 TCP 连接同步 UTF-8 纯文本剪贴板。
- 支持 Windows 原生 Unicode 剪贴板。
- 支持 Linux Wayland 的 `wl-clipboard`。
- 支持 Linux X11 的 `xclip` 或 `xsel`。
- 远端写入抑制，避免两台设备无限循环同步。
- 单条文本最大 1 MiB；超限内容不会发送。
- 连接中断后，主动连接端会自动重试。

目前不支持图片、文件、富文本和剪贴板历史。

## 环境要求

- Rust 1.85 或更高版本。
- Linux Wayland：安装 `wl-clipboard`。
- Linux X11：安装 `xclip` 或 `xsel`。
- Windows 无额外剪贴板依赖。

GNOME Wayland 还需要 ClipSync 自带的 Shell 扩展。首次安装：

```bash
cargo run --release -- setup-gnome
```

安装后注销并重新登录一次，然后启用扩展：

```bash
gnome-extensions enable clipsync@local
```

这是 GNOME 的平台限制：GNOME 不向普通后台程序提供全局剪贴板变化事件，必须由
Shell 扩展监听。扩展只在 ClipSync 运行时把纯文本交给本机会话 D-Bus 服务，
不会建立网络连接。

Ubuntu/Debian Wayland 可以运行：

```bash
sudo apt install wl-clipboard
```

## 构建

```bash
cargo build --release
```

生成的程序位于：

- Linux：`target/release/clipsync` 和 `target/release/clipsync-gui`
- Windows：`target\release\clipsync.exe` 和 `target\release\clipsync-gui.exe`

Tauri 界面与 CLI 传输程序是两个独立构建产物，GUI 后台服务会调用同目录中的
CLI 完成剪贴板同步。

## 局域网设备发现

在两台处于同一局域网的设备上分别运行：

```bash
cargo run --release -- discover "Linux laptop"
```

```powershell
cargo run --release -- discover "Windows desktop"
```

双方应在几秒内显示：

```text
+ Found Windows desktop [windows] at 192.0.2.42 ...
```

自动发现使用 IPv4 UDP 组播地址 `239.255.67.83:24871`。Windows 防火墙询问
时，需要允许当前网络配置文件访问 UDP `24871`。

### 校园网直接 IP 模式

如果校园网禁用了组播，一台设备可以只监听单播：

```bash
cargo run --release -- discover "Linux laptop" --no-multicast
```

另一台设备填写其 IPv4 地址：

```powershell
cargo run --release -- discover "Windows desktop" `
  --peer 192.0.2.41 --no-multicast
```

查询地址：

- Linux：`ip -4 addr`
- Windows：`ipconfig`

直接 IP 可以绕过组播过滤，但不能绕过完全禁止客户端互访的 AP 隔离。

## 文本剪贴板同步

发现成功后，选择其中一台设备监听同步连接。例如在 Linux 上运行：

```bash
cargo run --release -- sync "Linux laptop"
```

默认监听 TCP `0.0.0.0:24872`。然后在 Windows 上填写 Linux 的 IP：

```powershell
cargo run --release -- sync "Windows desktop" --peer 192.0.2.41
```

出现以下信息表示连接建立：

```text
Connected to 192.0.2.41:24872
Transport peer: Linux laptop [linux]
```

连接建立后，在任意一端复制文本，另一端即可直接使用 `Ctrl+V` 粘贴。同步是
双向的，不需要再建立反向连接。程序启动前已经存在的剪贴板内容不会自动发送，
只有启动后的变化会同步。

## Tauri 2 GUI

GUI 已使用 Tauri 2 重构，不需要任何启动参数。窗口中央显示本机 IPv4，其下方
是“开始连接”按钮和“开机自启”开关。连接时使用当前登录用户的名称：Windows
读取 `USERNAME`，Linux 读取 `USER`/`LOGNAME`。

构建 GUI：

```bash
cargo build --release -p clipsync-cli -p clipsync-gui
```

运行：

```bash
# Linux
target/release/clipsync-gui
```

```powershell
# Windows
target\release\clipsync-gui.exe
```

首次打开 GUI 会启动一个单实例后台服务；再次打开 GUI 不会重复启动服务，关闭
窗口也不会停止剪贴板同步。点击“开始连接”，输入另一台设备界面上显示的 IP
地址并确认即可连接。成功后按钮会显示对端用户名，并在本机配置目录保存最近成功
连接的 IP（最多 5 个）。后台服务下次启动会依次尝试这些地址；全部不可达时继续
监听。自动尝试期间按钮始终显示“开始连接”，不会显示正在连接或失败状态；只有
握手成功后才显示已连接的对端用户名。任意一端的底层 `clipsync` 进程退出或网络
连接断开时，另一端也会立即退出“已连接”状态。

“开机自启”开启后只自动启动后台同步服务，不弹出 GUI。Linux 写入当前用户的
`~/.config/autostart/clipsync.desktop`（尊重 `XDG_CONFIG_HOME`）；Windows 写入
当前用户的 `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run`。关闭开关
会移除对应注册。GUI 会调用同目录中的 `clipsync` 传输程序，因此分发时需要把
两个可执行文件放在一起。匹配码机制已经完全移除。

文档中的 `192.0.2.0/24` 地址属于专用文档示例网段，不是任何开发者或用户的真实
地址；实际使用时请填写另一台设备界面显示的局域网 IPv4。

Ubuntu/Debian 构建 Tauri GUI 前需要安装 WebKitGTK 开发依赖：

```bash
sudo apt install libwebkit2gtk-4.1-dev
```

Windows 使用系统 WebView2；Windows 10 1803 及之后的版本通常已经预装。

### 便携包

当前 GUI 通过同目录下的 `clipsync` 启动传输服务，因此两个程序必须一起分发。
先构建：

```bash
cargo build --release -p clipsync-cli -p clipsync-gui
```

Linux 可以创建包含两个程序和图标的压缩包：

```bash
mkdir -p dist/clipsync-linux
cp target/release/clipsync target/release/clipsync-gui dist/clipsync-linux/
cp crates/clipsync-gui/icons/icon.png dist/clipsync-linux/
tar -C dist -czf dist/clipsync-linux-x86_64.tar.gz clipsync-linux
```

Windows 请在 Windows 主机上构建，然后用 PowerShell 打包：

```powershell
New-Item -ItemType Directory -Force dist\clipsync-windows | Out-Null
Copy-Item target\release\clipsync.exe,target\release\clipsync-gui.exe,crates\clipsync-gui\icons\icon.ico dist\clipsync-windows\
Compress-Archive -Force dist\clipsync-windows\* dist\clipsync-windows-x86_64.zip
```

不要只复制 `clipsync-gui`，否则后台服务无法启动。

如需生成 Tauri 原生安装包，先安装 Tauri CLI：

```bash
cargo install tauri-cli --version "^2"
```

Linux 构建 `.deb` 和 `.AppImage`（sidecar 文件名必须带当前 target triple）：

```bash
cargo build --release -p clipsync-cli
target_triple="$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p crates/clipsync-gui/binaries
cp target/release/clipsync \
  "crates/clipsync-gui/binaries/clipsync-$target_triple"
cd crates/clipsync-gui
cargo tauri build --config tauri.linux.conf.json
```

产物位于工作区 `target/release/bundle/deb/` 和
`target/release/bundle/appimage/`。Windows 请在 Windows 主机的 PowerShell 中运行：

```powershell
cargo build --release -p clipsync-cli
$targetTriple = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ', '')
New-Item -ItemType Directory -Force crates\clipsync-gui\binaries | Out-Null
Copy-Item target\release\clipsync.exe "crates\clipsync-gui\binaries\clipsync-$targetTriple.exe"
Set-Location crates\clipsync-gui
cargo tauri build --config tauri.windows.conf.json
```

Windows 安装包位于工作区 `target\release\bundle\nsis\` 和
`target\release\bundle\msi\`。这些专用配置会把传输程序作为 sidecar 一并打包。

### 在 Linux 注册为桌面应用

下面以当前用户安装为例，不需要 `sudo`：

```bash
install -Dm755 target/release/clipsync "$HOME/.local/lib/clipsync/clipsync"
install -Dm755 target/release/clipsync-gui "$HOME/.local/lib/clipsync/clipsync-gui"
install -Dm644 crates/clipsync-gui/icons/icon.png \
  "$HOME/.local/share/icons/hicolor/1254x1254/apps/clipsync.png"
mkdir -p "$HOME/.local/share/applications"
```

创建 `~/.local/share/applications/com.clipsync.desktop.desktop`：

```desktop
[Desktop Entry]
Type=Application
Name=ClipSync
Comment=局域网剪贴板同步
Exec=/home/你的用户名/.local/lib/clipsync/clipsync-gui
Icon=clipsync
Terminal=false
Categories=Utility;
StartupWMClass=com.clipsync.desktop
```

把 `Exec` 中的“你的用户名”换成当前 Linux 用户名，然后执行：

```bash
chmod 644 "$HOME/.local/share/applications/com.clipsync.desktop.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
```

此后可以从应用菜单搜索 ClipSync 并固定到 Dock。GUI 内的“开机自启”会记录
当前已安装的 `clipsync-gui` 路径，所以应在完成安装后再开启该开关。

Linux 监听策略：

- GNOME Wayland：Shell 扩展事件 + 会话 D-Bus，无轮询。
- 支持 data-control 的其他 Wayland 桌面：单个长驻 `wl-paste --watch`。
- X11：`xclip`/`xsel` 后备监听。

旧版本每 350 ms 启动一次 `wl-paste`，会造成 GNOME Dock 图标反复闪烁、干扰
输入法，并可能因某次读取阻塞而停止发送。当前版本已移除 GNOME 上的进程轮询。

自定义监听端口：

```bash
cargo run --release -- sync "Linux laptop" --port 25000
```

连接自定义端口：

```powershell
cargo run --release -- sync "Windows desktop" --peer 192.0.2.41:25000
```

### 防火墙

监听端需要允许 TCP `24872` 入站。校园网通常会被 Windows 识别为“公用网络”，
需要确认规则适用于当前网络配置文件。

### 当前安全限制

当前文本同步 MVP 尚未实现身份认证和传输加密，剪贴板文本会以未加密形式通过
指定的 TCP 连接传输。请只在自己控制的设备和可信网络中使用，不要用于同步
密码、令牌、私钥等敏感内容。

后续阶段会加入持久设备身份、双方验证码确认、密钥交换和认证加密。届时设备的
IP 地址只用于定位，不再作为信任依据。

## 开发路线

1. 局域网组播与直接 IP 发现：已完成。
2. 双向纯文本剪贴板同步 MVP：当前阶段。
3. 设备配对、身份认证和加密传输。
4. 扩展更多原生事件监听与桌面环境兼容性。
5. 图片、文件、托盘界面和安装包；后台常驻与开机启动已完成。

## 开发检查

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
