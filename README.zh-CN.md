# Never Sleep

[English](README.md) | [简体中文](README.zh-CN.md) · **[网站](https://xyz-ai.app/never-sleep/zh/)**

让 MacBook **屏幕关掉、电脑不睡**。挂机下载、当成迷你服务器、远程会话，或只是护屏省电——菜单栏一点即可，也提供给 Agent 用的命令行。

界面 **默认英语**；系统语言为中文时（或在菜单里选择「简体中文」）使用中文。

## 菜单栏操作

左键点一下菜单栏的太阳即可打开紧凑面板（硬币、状态、开始按钮）。

1. 点 **开始关屏待命**。硬币翻成月亮，下方显示已过时长（有限时长则为剩余倒计时），约 1.5 秒后屏幕关闭。
2. **结束待命** 始终可点；**立即熄屏** 会马上关屏，不结束待命。
3. 关掉面板或再点图标只是隐藏，不会结束待命。
4. 时长、关屏、语言和其他开关都在更多设置里；内置使用说明一点就懂。

## 适用场景

机制只有一件：**屏幕关掉，电脑不睡**。这件事不只服务于 ChatGPT。

- **挂机下载** — 大文件、App Store 更新、Time Machine 可以整夜跑。屏幕关掉，传输不停。
- **当成一台迷你服务器** — SSH、文件共享、本地站点或家庭实验室，更像一台 Mac mini。**最稳妥仍是开盖熄屏**；合盖保活是尽力而为。
- **护屏** — 真·显示器休眠，不是把亮度拉到 0。少发热、少损耗，桌上也更暗。
- **降低功耗** — 背光是 MacBook 空闲功耗的大头。关掉屏幕，CPU、磁盘和网络仍可用。
- **远程会话** — ChatGPT、Codex、Cursor、屏幕共享或 SSH。远程输入把屏幕点亮也不怕，人不在时会再关掉。
- **长时间任务不用守着** — 通宵 3D 渲染、视频导出、编译、转码、备份、同步。和防休眠工具要干的是同一类活，只是屏幕关掉。1/3/8 小时或到 08:00，加上电量下限。

## 为什么做这个，而不是再用一遍防休眠工具

「永远醒着」这类工具 — Caffeine、Amphetamine、Don't Sleep、模拟鼠标 — 是为了让活永远不停：通宵渲染、编译、远程桌面、挂机脚本。做法是每隔几秒轻微移动鼠标，或通知系统 **不要关屏**。电脑是醒着的，一块常亮、画面几乎不动的屏幕也是。

PowerToys Awake 在 Windows 上是同一类活，但「保持屏幕打开」是可选项。

痛点在这里：

- **屏幕一直亮着。** OLED 烧屏、多余发热、浪费背光，空桌子上一台发光的电脑。
- **「允许关屏」藏得很深**，有的根本没有。默认是屏幕也别关（`caffeinate -d`、「保持屏幕打开」）。
- **模拟键鼠是在撒谎。** 鼠标微动看起来像有人在。Never Sleep 不模拟鼠标或键盘，所以不会让 Teams 一直「可用」。
- **远程会话把屏幕点亮后，没有人再关掉。**
- **有的工具会改系统节能设置**（`pmset`），崩溃后电脑再也睡不着。

这个产品做的是另一件事：

> 人走了，屏幕必须灭（护屏、省电、隐私）；机器必须醒（下载、迷你服务器、远程会话）。

体验上的硬指标：

1. **一键进入**：菜单第一项就是「开始关屏待命」，1.5 秒后关屏，系统保持运行。
2. **不跟人抢屏幕**：你坐在电脑前敲键盘，绝不强制关屏；HID 空闲一段时间或合上盖，再自动关。
3. **远程操作把屏幕点亮也不怕**：人不在时周期性重申 `displaysleepnow`。远程会话用合成事件点亮屏幕时，几秒内会再灭掉。
4. **回来的路永远在**：全局快捷键 `⌥⌘P`（屏幕灭了也能按），或再点菜单。
5. **不改系统节能设置**：只用进程内 IOKit 断言。退出、崩溃、再次启动会还原合盖标志，不会留下 `pmset disablesleep 1` 这种开机还在的坑。
6. **Agent 友好**：同一套状态可被 `never-sleep status --json` 读取，Codex 可以自己 `never-sleep on --for 8h`。

## 用法

### 菜单栏（推荐）

在 Mac 上：

```bash
cargo build -p never-sleep --release
./scripts/package-macos.sh
open "dist/Never Sleep.app"
```

菜单栏默认显示拟人化太阳图标。左键打开面板，再点 **开始关屏待命**。硬币会翻成月亮，约 1.5 秒后屏幕关闭。点 **结束待命** 即可停止。隐藏面板（或再点图标）不会结束待命。右键图标仍可打开原生备用菜单。Finder 显示 **Never Sleep**。

| 选项 | 默认 | 含义 |
| --- | --- | --- |
| 立即关闭屏幕 | 开 | 主功能，真·显示器休眠，不是把亮度拉到 0 |
| 合盖尽量保持运行 | 开 | 尽力而为；**最稳妥仍是开盖熄屏** |
| 人离开后自动再关屏 | 开 | 远程代理误亮屏幕时盖回去 |
| 关屏时锁定登录 | 关 | GUI 远程操控需要解锁会话，所以默认关 |
| 电量低于 20% 时结束 | 开 | 避免在背包里把电池耗干 |
| 时长 | 无限期 | 也可 1/3/8 小时，或到当天 08:00 |
| 语言 | 中文系统为中文，否则英语 | English / 简体中文；`--lang` 与 `NEVER_SLEEP_LANG` 可覆盖 |

### 命令行

菜单栏运行时，命令会打到同一个进程：

```bash
never-sleep on --for 8h
never-sleep status --json
never-sleep off
never-sleep doctor      # 看断言、电池、合盖
never-sleep cleanup     # 进程异常退出后的保险
never-sleep explain
never-sleep --lang zh status
```

SSH 上没有菜单栏时，`never-sleep on` 会以前台方式占用该进程（类似 `caffeinate`），Ctrl-C 结束。

给 Agent 的最小片段：

```bash
never-sleep on --for 8h
# …长时间任务…
never-sleep off
```

### 语言

优先级（最后才落到英语）：

1. 本进程 `--lang en|zh` 或 `NEVER_SLEEP_LANG=en|zh`
2. 菜单「语言」里的选择（写入 `config.toml`）
3. macOS 首选语言 / Unix `LANG`
4. **英语**

JSON 输出始终为英语，方便 Agent 使用稳定字段。

## 手机看板

在手机上查看每台已配对 Mac 的实时状态，并远程开始或结束关屏待命。打开 **[手机看板](https://xyz-ai.app/never-sleep/zh/board/)**。

远程访问默认关闭，包括从旧版本升级但尚未设置开关的用户。关闭时客户端完全在本地运行：不创建远程身份、不建立云端连接、不发送心跳，也不展示配对码或二维码。本地待命、快捷键和 CLI 照常工作。

1. 在 Mac 上打开 **更多设置**，手动开启 **远程访问**，再点同一行的 **配对** 查看二维码和配对码。（开启后也可以运行 `never-sleep pair`。）
2. 在手机里输入该配对码，或打开 App 给出的配对链接。同一个浏览器可以同时看多台电脑。
3. 列表是实时的：在线/离线（最近心跳）、待命开/关、屏幕已关或亮着、开盖/合盖、电源或电量、剩余时长、以及机器名称。
4. **开始关屏待命** / **结束待命** 只作用于你点的那一台，不会对整组电脑广播。手机用该机的配对令牌鉴权。Mac 走和菜单栏相同的本地 `on` / `off` Engine，不改写节能设置。
5. 若 Mac 离线，看板会说明指令没有生效，不会假装状态已变。远程开始待命时，人在键盘前仍然不会强制关屏。
6. **立即熄屏** 是明确的关屏请求，即使近期有键鼠活动或当前未待命也可使用；不改变待命状态和计时。指令优先通过长连接即时推送；回退到 HTTP 时可能需要约 10 秒再加网络耗时；Mac 应用和 Worker 都需要更新。远程开始待命检测到近期输入时会推迟自动熄屏。

开关会保存到 `config.toml` 的 `remote_enabled` 字段。关闭后断开现有连接，不再发送收尾心跳；已发出的请求可能需要片刻结束，手机看板在约 35 秒内显示离线。已保存的配对身份会保留，重新开启后可继续使用。关闭期间 `never-sleep pair --json` 返回 `remote_disabled`，不会为了配对而自动联网。

Mac 优先使用经过鉴权的 WebSocket 连接。状态变化即时上报（最多每秒合并发送一次），每五分钟完整同步一次。每 10 秒发送轻量保活，由 Cloudflare 自动应答，不唤醒 Durable Object，也不写入存储。远程指令即时推送，保留到执行确认，60 秒未执行则过期。连接使用 Durable Objects 休眠 API，空闲连接不会让对象持续运行。

手机看板仅在页面可见时订阅，每台可见设备一条连接；每 20 秒进行一次只读在线检查，不写心跳记录，倒计时在本地计算。WebSocket 不可用时，Mac 回退为每 10 秒一次 HTTP 心跳，手机回退为每 30 秒轮询列表；重连按退避策略延长到五分钟。最后联系超过 35 秒会判为离线；退出时明确上报离线。前台与菜单栏进程交接后会重新连接并确认待执行命令。

请先部署 Worker、限流绑定及网站文件，再更新 Mac 客户端；仍兼容旧 HTTP 客户端。入口原生限流按 Cloudflare 节点近似计数，鉴权后的设备限额保留在设备 DO 内；缺少任一绑定的自定义部署会回退到旧限流 DO。限流 namespace ID 需在账号内唯一。WebSocket token 放在握手子协议请求头中，不放在 URL 中。比较成本时需分别观察请求量、运行时长及存储用量。

## 技术方案

电源语义在 macOS 上是拆开的，这是本应用能「关屏 + 不睡」的前提：

| 能力 | 做法 | 作用范围 |
| --- | --- | --- |
| 阻止空闲睡眠 | `PreventUserIdleSystemSleep` | 官方、进程级、关屏仍允许 |
| 阻止磁盘休眠 | `PreventDiskIdle` | 远程读写更稳 |
| 保持网络 | `NetworkClientActive` | 降低 Wi-Fi 打盹 |
| 关屏 | `pmset displaysleepnow`，失败则 `IODisplayWrangler IORequestIdle` | 不阻止系统睡眠 |
| 合盖尽力 | `PreventSystemSleep`（主要 AC）+ RootDomain 选择器 12 关闭 clamshell sleep + `CanSystemSleep` 时 `IOCancelPowerChange` | **不保证**所有机型/系统版本 |
| 看门狗 | 人不在则每 3 秒重申关屏 | 对付远程 HID/合成输入 |
| 「人在不在」 | `IOHIDSystem HIDIdleTime` + 合盖状态 | 合成事件通常不重置 HID 空闲，正好 |

刻意不做的事：

- **不**默认执行 `sudo pmset -a disablesleep 1`。它会写进系统偏好，重启后还在，App 崩了电脑就再也不睡。
- **不**使用 `PreventUserIdleDisplaySleep` / `caffeinate -d`，那会让屏幕一直亮着，和护屏目标相反。

合盖说明（请务必读）：Apple 在无外接屏时合盖倾向于整机睡眠，这是散热设计。选择器 12 在部分 Apple Silicon + 较新系统上有效，在更新的系统上可能被无视。UI 里写的是「尽量」，诊断命令 `never-sleep doctor` 可核对 `pmset -g assertions`。护屏主路径是 **开盖 + 显示器休眠**，这是 IOKit 官方支持、也最护屏的组合。

安全网：

- 电池低于阈值（未插电）自动结束待命
- 过热 `Critical` 时结束待命
- `~/Library/Application Support/Never Sleep/session.lock` 记录 pid；下次启动发现进程已死会还原合盖标志
- panic hook 同样还原

架构：

```
never-sleep-core   纯策略（可在 Linux 单测）
never-sleep        CLI + macOS 菜单栏
```

引擎只输出 `ApplyPower` / `SleepDisplay` / `LockSession` / `Notify`，平台层负责 IOKit。这样关屏策略不依赖本机能不能编译 AppKit。

## 与常见工具对比

| | Never Sleep | caffeinate | KeepingYouAwake | Amphetamine |
| --- | --- | --- | --- | --- |
| 默认关屏 | 是，还强制关 | `-i` 才允许关屏 | 否 | 需改会话选项 |
| 远程点亮后再关 | 是 | 否 | 否 | 否 |
| 人在电脑前不抢屏 | 是 | 否 | 否 | 否 |
| 合盖 | 尽力而为 | `-s` 仅 AC | 明确不支持 | 较强，常要 Enhancer |
| 改系统 pmset | 否 | 否 | 否 | 部分模式会 |
| JSON / Agent CLI | 是 | 否 | 否 | 否 |

## 开发

本仓库采用 **测试先行（TDD）**。流程、不变量和测试应写在哪里，见 [AGENTS.md](AGENTS.md)。

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # Linux / Mac 都可跑核心测试
# 菜单栏与 IOKit 只在 macOS 链接
cargo build -p never-sleep --release   # 请在 Mac 上
```

配置文件：`~/Library/Application Support/Never Sleep/config.toml`  
IPC 套接字：同目录 `ipc.sock`

要求 **Rust 1.88+**、macOS 12+。菜单栏以 `LSUIElement` 运行，不占 Dock。

## 许可

MIT
