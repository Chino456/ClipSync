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

GUI 会在后台调用同目录中的 `clipsync`，因此分发时需要把这两个文件放在一起。

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
+ Found Windows desktop [windows] at 10.20.30.42 ...
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
  --peer 10.20.30.41 --no-multicast
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
cargo run --release -- sync "Windows desktop" --peer 10.20.30.41
```

出现以下信息表示连接建立：

```text
Connected to 10.20.30.41:24872
Transport peer: Linux laptop [linux]
```

连接建立后，在任意一端复制文本，另一端即可直接使用 `Ctrl+V` 粘贴。同步是
双向的，不需要再建立反向连接。程序启动前已经存在的剪贴板内容不会自动发送，
只有启动后的变化会同步。

## 单按钮 GUI

GUI 保留了 `sync` 命令的参数，但窗口中只有一个按钮。第一次点击“开始连接”会
启动同步，再次点击“停止连接”会结束同步及其剪贴板监听进程；关闭窗口也会自动
停止。

例如，让 Linux 作为监听端：

```bash
target/release/clipsync-gui "Linux laptop"
```

让 Windows 通过 Linux 的 IP 主动连接：

```powershell
target\release\clipsync-gui.exe "Windows desktop" --peer 10.20.30.41
```

也可以把 `--port`、带端口的 `--peer` 等原有参数原样传给 GUI。Linux 使用系统
GTK 3，Windows 使用原生 Win32 窗口，不需要额外安装 GUI 框架。

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
cargo run --release -- sync "Windows desktop" --peer 10.20.30.41:25000
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
5. 图片、文件、托盘界面、开机启动和安装包。

## 开发检查

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
