# Windows 多显示器 DPI 缩放问题

## 症状

在多显示器环境下，将 zenterm 窗口从一个显示器拖拽到另一个不同缩放倍率的显示器时，窗口无法正确缩放：
- 窗口物理尺寸变小/变大
- 终端网格行列数不正确
- 字体渲染比例错误

Alacritty（另一个基于 winit 的 Rust 终端）也存在完全相同的 bug。

## 根本原因

问题不在 zenterm，而在其底层的窗口库 **winit 0.30.x** 的 Windows 后端的 DPI 处理。

### winit 的 `WM_DPICHANGED` 处理器有 bug

当窗口被拖拽到不同 DPI 的显示器时，Windows 发送 `WM_DPICHANGED` 消息。winit 的处理代码中使用了`MonitorFromWindow()` 来确定目标显示器：

```rust
// winit/src/platform_impl/windows/event_loop.rs 约 2460 行
let new_dpi_monitor = unsafe {
    MonitorFromWindow(window, MONITOR_DEFAULTTONULL) // ← BUG
};
```

**问题**：`WM_DPICHANGED` 在窗口**移动过程中**发送，此时 `MonitorFromWindow()` 返回的还是**旧显示器**（窗口尚未物理移动）。winit 根据这个错误的结果运行了一段"位置验证"代码，把窗口推回旧显示器位置，这立即触发了另一个 `WM_DPICHANGED`——形成**无限 DPI ping-pong**，窗口在两个显示器之间来回弹跳。

### 正确做法

Windows 文档要求的做法是使用 `WM_DPICHANGED` 的 `lparam` 参数——它指向一个**建议矩形**（`RECT`），由 OS 计算得出，能保持窗口的逻辑尺寸不变：

```rust
// ✅ 正确的修复
let suggested_rect = *(lparam as *const RECT);
let new_dpi_monitor = unsafe {
    MonitorFromRect(&suggested_rect, MONITOR_DEFAULTTONULL)
};
```

### 受影响的项目

所有基于 winit 的 Rust 程序在 Windows 多显示器下都有此问题，包括：
- **Alacritty**（知名 Rust 终端）
- **任何 eframe/egui 应用**
- **任何 Tauri 应用**（底层使用 TAO/winit）

## wezterm 为什么正常

wezterm **不使用 winit**。它拥有自己的 Win32 窗口过程（`WNDPROC`），直接用 Win32 API 处理窗口消息。关键差异：

| | wezterm | 基于 winit 的程序 |
|---|---|---|
| 窗口过程 | 自建 `WNDPROC` | winit 生成的 `WNDPROC` |
| DPI 事件源 | `WM_WINDOWPOSCHANGED`（移动完成后） | `WM_DPICHANGED`（移动过程中） |
| DPI 读取 | `GetDpiForWindow(hwnd)`（当前实时值） | 从消息 `wparam` 提取 |
| 显示器识别 | 不需要——只读窗口当前状态 | `MonitorFromWindow()` → **有 bug** |

## 已尝试的修复方案（均未成功）

### 1. 窗口子类化 + 预应用建议矩形

拦截 `WM_DPICHANGED`，先调用 `SetWindowPos` 应用建议矩形，再转发给 winit。

**失败原因**：winit 收到消息后会再次调用 `SetWindowPos` 用自己的计算覆盖我们的设置，导致窗口尺寸不正确。

### 2. `GetDpiForWindow` 轮询 + 稳定门控

每帧调用 `GetDpiForWindow()` 获取真实 DPI，连续 3 帧相同才提交。

**失败原因**：winit 的 ping-pong 导致 DPI 每帧都在两个值之间切换，稳定门控永远达不到阈值。

### 3. 振荡检测 + 防抖

检测 DPI 在两个值之间交替振荡，振荡 3 次后提交目标 DPI，随后进入 500ms 防抖期。

**失败原因**：ping-pong 持续不断（用户拖拽过程中 winit 持续弹跳窗口），防抖过期后又再次触发，无法收敛。

## 已知修复方案

### 方案 A：使用 fork 的 winit（推荐）

修改 winit 源码中的两处：

1. `MonitorFromWindow` → `MonitorFromRect`（1 行）
2. 删除窗口位置验证代码（约 20 行）

通过 `[patch.crates-io]` 在 `Cargo.toml` 中引入：

```toml
[patch.crates-io]
winit = { git = "https://github.com/your-fork/winit", branch = "zenterm-0.30.x" }
```

每次 winit 发版时合并上游即可，预计每次维护工作量约 5 分钟。

### 方案 B：替换 winit

自己实现窗口栈（如 wezterm 所做）。工程量巨大（键盘/鼠标/IME/剪贴板/多平台），对当前阶段不现实。

### 方案 C：等待上游修复

winit 社区已知此问题：
- [winit#3040](https://github.com/rust-windowing/winit/issues/3040)（2022年报，核心 bug）
- [winit#4600](https://github.com/rust-windowing/winit/issues/4600)（2026年6月，确认所有 winit 应用受影响）
- [egui#7648](https://github.com/emilk/egui/issues/7648)（2025年10月，egui 层面的跟踪）

修复 PR 已被提过多次（#4119, #4341），但因维护者内部对"要不要保留位置验证代码"有分歧，尚未合入。

## Windows-sys 相关 API 速查

与 DPI 相关的 Win32 API 及它们在 `windows-sys 0.61` 中的位置：

| API | Feature | 说明 |
|-----|---------|------|
| `GetDpiForWindow(hwnd)` | `Win32_UI_HiDpi` | 返回窗口当前所在显示器的 DPI |
| `MonitorFromRect(rect, flags)` | `Win32_UI_WindowsAndMessaging` | 从矩形确定显示器 |
| `MonitorFromWindow(hwnd, flags)` | `Win32_UI_WindowsAndMessaging` | 从窗口确定显示器（**winit bug 所在**） |
| `GetActiveWindow()` | `Win32_UI_Input_KeyboardAndMouse` | 获取当前活动窗口句柄 |
| `SetWindowPos()` | `Win32_UI_WindowsAndMessaging` | 设置窗口位置和大小 |

## 参考资料

- [winit issue #3040 — Incorrect DPI scaling on Windows](https://github.com/rust-windowing/winit/issues/3040)
- [winit issue #4600 — DPI scaling broken when dragging between monitors](https://github.com/rust-windowing/winit/issues/4600)
- [winit PR #4119 — prevent incorrect shifting of window (closed)](https://github.com/rust-windowing/winit/pull/4119)
- [winit PR #4341 — DPI fix attempt](https://github.com/rust-windowing/winit/pull/4341)
- [egui issue #7648 — Improper resizing when dragging windows](https://github.com/emilk/egui/issues/7648)
- [Chromium WM_DPICHANGED handler（参考实现）](
  https://source.chromium.org/chromium/chromium/src/+/main:ui/views/win/hwnd_message_handler.cc)
