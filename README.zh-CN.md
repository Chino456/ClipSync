# ClipSync

简体中文 | [English](README.md)

ClipSync 是一个逐步开发的跨平台剪贴板同步工具，用于在同一局域网内的
Windows 和 Linux 设备之间同步剪贴板。

## 当前能力

- UDP 组播自动发现局域网设备。
- 在禁用组播的校园网中通过 IPv4 地址直接发现设备。
- 通过一条双向 TCP 连接同步 UTF-8 纯文本和 PNG 图片剪贴板。
- 支持 Windows 原生 Unicode、PNG 和 CF_DIBV5 图片剪贴板。
- 支持 Linux Wayland 的 `wl-clipboard`。
- 支持 Linux X11 的 `xclip`；`xsel` 只能同步文本。
- 远端写入抑制，避免两台设备无限循环同步。
- 单条文本最大 1 MiB，PNG 图片最大 10 MiB；超限内容不会发送。
- 连接中断后，主动连接端会自动重试。

目前不支持文件、富文本和剪贴板历史。

## 环境要求

- Rust 1.85 或更高版本。
- Linux Wayland：安装 `wl-clipboard`。
- Linux X11：图片同步必须安装 `xclip`；`xsel` 仅作为文本后备。
- Windows 无额外剪贴板依赖。

GNOME Wayland 还需要 ClipSync 自带的 Shell 扩展。首次安装：

```bash
cargo run --release -- setup-gnome
```

安装后注销并重新登录一次，然后启用扩展：

```bash
gnome-extensions enable clipsync@local
```

从 0.1.x 升级到 0.2.0 时需要重新运行 `clipsync setup-gnome` 覆盖扩展文件，并
注销后重新登录；扩展 v2 才能监听 PNG 图片。

这是 GNOME 的平台限制：GNOME 不向普通后台程序提供全局剪贴板变化事件，必须由
Shell 扩展监听。扩展只在 ClipSync 运行时把纯文本交给本机会话 D-Bus 服务；图片
由两个长驻的 `wl-paste --watch` 进程分别监听文本和 PNG 图片。两者都不会自行建立网络连接。

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

## 文本与图片剪贴板同步

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

连接建立后，在任意一端复制文本或图片，另一端即可直接使用 `Ctrl+V` 粘贴。
图片在网络中统一以 PNG 传输，单张最大 10 MiB。同步是双向的，不需要再建立
反向连接。程序启动前已经存在的剪贴板内容不会自动发送，只有启动后的变化会同步。

图片同步使用协议 v2，两台设备都必须升级到 ClipSync 0.2.0 或更高版本；v1 与 v2
会在握手阶段拒绝连接，不会静默发送不兼容数据。

由于后台服务在 GUI 关闭后仍会常驻，从 0.1.x 升级前应先在两台设备彻底结束旧的
`clipsync` 和 `clipsync-gui --daemon` 进程，或直接重启系统，再安装并启动 0.2.0。
否则内存中仍在运行的 v1 服务不会自动变成 v2。

## Tauri 2 GUI

GUI 已使用 Tauri 2 重构，不需要任何启动参数。窗口中央显示本机 IPv4，其下方
是“开始连接”按钮、“开机自启”开关和“结束进程”按钮。连接时使用当前登录用户
的名称：Windows 读取 `USERNAME`，Linux 读取 `USER`/`LOGNAME`。

“结束进程”会关闭当前剪贴板传输进程和后台守护进程，确认控制端口已经关闭后显示
“进程已结束”。GUI 本身保持打开，状态轮询不会自动重启服务；再次输入有效 IP 并
点击连接时，GUI 会重新启动后台服务并发起连接。该按钮不会修改“开机自启”设置。

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
`target/release/bundle/appimage/`。

### 在 Windows 生成安装包

Windows 安装包必须在 Windows 主机上构建，推荐使用 64 位 Windows 10/11。需要：

- Rust MSVC 工具链，可用 `rustup default stable-x86_64-pc-windows-msvc` 设置。
- Microsoft C++ Build Tools，并勾选“使用 C++ 的桌面开发”和 Windows SDK。
- WebView2 Runtime；Windows 10/11 通常已经安装。
- Tauri CLI 2：`cargo install tauri-cli --version "^2"`。
- 首次生成 NSIS/MSI 时需要联网下载相应的打包工具。

在项目根目录打开 PowerShell，依次运行：

```powershell
cargo build --release -p clipsync-cli
$targetTriple = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ', '')
New-Item -ItemType Directory -Force crates\clipsync-gui\binaries | Out-Null
Copy-Item target\release\clipsync.exe "crates\clipsync-gui\binaries\clipsync-$targetTriple.exe"
Set-Location crates\clipsync-gui
cargo tauri build --config tauri.windows.conf.json
```

`tauri.windows.conf.json` 会生成两种安装包，并把 `clipsync.exe` 作为 sidecar 一并
安装，不能省略上述复制和重命名步骤：

- NSIS `.exe`：工作区 `target\release\bundle\nsis\`。
- MSI：工作区 `target\release\bundle\msi\`。

如果本机无法构建 MSI，可以先只生成更通用的 NSIS 安装程序：

```powershell
cargo tauri build --config tauri.windows.conf.json --bundles nsis
```

安装后从开始菜单运行 ClipSync。第一次监听 TCP `24872` 时，Windows 防火墙可能
询问是否允许网络访问；同一热点或可信局域网通常勾选“专用网络”，校园网被识别为
公用网络时需要同时确认对应防火墙配置。正式公开发布前还应使用代码签名证书为
安装程序和两个 `.exe` 签名，否则 Windows SmartScreen 可能显示未知发布者警告。

若不需要安装程序，可使用前面的 ZIP 便携包；必须保持 `clipsync.exe` 与
`clipsync-gui.exe` 位于同一目录。

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
- 支持 data-control 的其他 Wayland 桌面：两个长驻 `wl-paste --watch`，分别监听文本和 PNG 图片。
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

当前同步协议尚未实现身份认证和传输加密，剪贴板文本和图片会以未加密形式通过
指定的 TCP 连接传输。请只在自己控制的设备和可信网络中使用，不要用于同步密码、
令牌、私钥、敏感截图等内容。

后续阶段会加入持久设备身份、双方验证码确认、密钥交换和认证加密。届时设备的
IP 地址只用于定位，不再作为信任依据。

## 开发路线

1. 局域网组播与直接 IP 发现：已完成。
2. 双向文本剪贴板同步：已完成。
3. 设备配对、身份认证和加密传输。
4. 扩展更多原生事件监听与桌面环境兼容性。
5. PNG 图片同步、后台常驻和开机启动：已完成；文件、托盘界面待开发。

## 发布 0.2.0

本版本将同步协议从 v1 升级到 v2，并加入 PNG 图片同步。发布前先确认版本号：

- 工作区 `Cargo.toml`：`0.2.0`。
- `crates/clipsync-gui/tauri.conf.json`：`0.2.0`。
- `Cargo.lock` 中三个工作区包均为 `0.2.0`。

执行完整检查并分别在 Linux、Windows 主机生成安装包：

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p clipsync-cli -p clipsync-gui
```

Linux 和 Windows 的 sidecar 打包命令见前面的打包章节。建议至少验收：

1. Windows → Linux 与 Linux → Windows 的文本复制。
2. 两个方向的截图和带透明通道 PNG 复制。
3. 超过 10 MiB 的 PNG 被拒绝且连接保持正常。
4. 结束任意一端 `clipsync` 后，另一端退出“已连接”状态。
5. `.deb`/`.AppImage`、NSIS/MSI 安装后都能找到同包内的 sidecar。
6. GNOME Wayland 已重新安装并启用 v2 Shell 扩展。

升级测试前先停止 0.1.x 常驻服务。Windows 可在任务管理器结束 ClipSync，Linux
可在退出 GUI 后运行 `pkill -x clipsync` 和 `pkill -f 'clipsync-gui --daemon'`。

验收通过后提交并创建标签：

```bash
git add Cargo.toml Cargo.lock README.md README.zh-CN.md crates gnome-extension
git commit -m "release: ClipSync 0.2.0 with image clipboard sync"
git tag -a v0.2.0 -m "ClipSync 0.2.0"
git push origin HEAD
git push origin v0.2.0
```

随后在 GitHub Releases 创建 `v0.2.0`，上传 Linux 的 `.deb`、`.AppImage` 和
Windows 的 NSIS `.exe`、`.msi`。如果使用 GitHub CLI：

```bash
gh release create v0.2.0 \
  target/release/bundle/deb/*.deb \
  target/release/bundle/appimage/*.AppImage \
  target/release/bundle/nsis/*.exe \
  target/release/bundle/msi/*.msi \
  --title "ClipSync 0.2.0" \
  --notes "新增 Windows/Linux 双向 PNG 图片剪贴板同步；同步协议升级至 v2。"
```

Linux 与 Windows 安装包需要在各自系统生成，再汇总到同一个 Release。仓库旧提交
历史曾包含真实局域网 IP；公开推送完整历史前应先清理或压缩历史，不能只检查当前
工作树。

## 开发检查

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
