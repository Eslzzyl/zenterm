# Zenterm vs tty7 对比分析

> 分析日期：2026-07-08
> tty7 版本：v0.6.2（项目年龄约 2 天）
> zenterm 版本：workspace（9 crates，持续开发数月）

---

## 一、概览

| 维度    | zenterm                             | tty7                           |
| ------- | ----------------------------------- | ------------------------------ |
| UI 框架 | egui 0.34 (immediate mode)          | GPUI (retained mode, Zed 生态) |
| 渲染    | 自定义 wgpu pipeline (WGSL shader)  | GPUI Element API               |
| 架构    | 单进程                              | 双进程（daemon + GUI）         |
| VT 核心 | alacritty_terminal 0.26 (crates.io) | alacritty_terminal (Zed fork)  |
| PTY     | portable-pty 0.9                    | portable-pty 0.8               |
| 字体    | cosmic-text + swash (纯 Rust)       | GPUI text_system               |
| 代码量  | ~12k 行 (9 crates)                  | ~29k 行 (4 modules)            |

---

## 二、性能对比

### 2.1 tty7 的宣示数据

| Benchmark     | tty7        | Alacritty | Ghostty | Kitty   |
| ------------- | ----------- | --------- | ------- | ------- |
| 11 MB cat     | **95 ms**   | 239 ms    | 179 ms  | 185 ms  |
| DOOM-fire FPS | **888 fps** | 485 fps   | 552 fps | 617 fps |
| 冷启动内存    | 116 MB      | 105 MB    | 128 MB  | 130 MB  |

来源：tty7 README，M1 Pro，155×40 grid，2026-07-04。

### 2.2 性能优势来源分析

tty7 的性能优势来自以下几个具体实现，**其中大部分与 GPUI 无关**：

| 优化手段                                                                       | 归属   | zenterm 状态                  |
| ------------------------------------------------------------------------------ | ------ | ----------------------------- |
| **Output batching** — 连续 Output 帧合并为一次 parser pass，减少 `Term` 锁竞争 | 终端层 | ✅ 已实现                     |
| **Thread-local grid buffer 复用** — 避免每帧分配 `RenderCell` 数组             | 渲染层 | ✅ 已实现                     |
| **Socket buffer 调大** (8 KiB → 256 KiB)                                       | IPC 层 | N/A (单进程)                  |
| **Atomic backpressure** — 16 MiB 高水位暂停 PTY reader                         | IPC 层 | N/A (单进程)                  |
| **SIMD OSC 分词器** — `memchr` 加速 Ground/Ignore 状态                         | 终端层 | ✅ 已实现                     |
| **Thin LTO + codegen-units=1**                                                 | 构建   | ✅ 已实现                     |
| **线程 QoS** — macOS USER_INTERACTIVE                                          | 平台   | ✅ 可通过配置实现             |
| **跳过 Selection/Search overlay passes** — 无高亮时跳过                        | 渲染层 | ✅ 已有类似（cache hit 跳过） |
| **CJK 批处理** — 连续 CJK 合并为 batch                                         | 渲染层 | ❌ 但影响面小                 |
| **GPUI 框架本身**                                                              | UI 层  | ❌ 无关（见 2.3）             |

### 2.3 关键结论：egui ≠ 慢

终端渲染的瓶颈链路是：

```
PTY read → VT parse → grid traversal → glyph lookup → instance build → GPU upload → draw call
```

其中 **UI 框架（egui/GPUI）只影响最后两步**。而 zenterm 已经通过 `egui_wgpu::CallbackTrait` 将终端网格完全交由自定义 wgpu pipeline 渲染，单次 instanced draw call 完成整个网格，**绕过了 egui 的 immediate mode overhead**。

egui 的 immediate mode overhead 影响的是 UI chrome（tab bar、sidebar、settings panel），这部分在 zenterm 中预算为 < 0.5ms，不在终端渲染的 hot path 上。

换句话说：**把 egui 换成 GPUI 不会让 cat 变快，也不会让 DOOM-fire 帧率翻倍。**

### 2.4 tty7 的 DOOM-fire 优势是否真实

DOOM-fire benchmark 测量的是**全屏逐帧刷新**场景——每个像素每帧都在变化。这测试的是 GPU draw call 数量和纹理上传带宽。

Alacritty 在此场景下的弱点在于它使用**双 Pass + Dual-Source Blending**：

- Pass 0：渲染背景
- Pass 1：渲染前景文字（依赖 Pass 0 的输出做混合）
- 两遍渲染意味着两倍的 vertex processing + fragment processing

而 tty7（以及 zenterm）使用**单 Pass**：

- 一个 draw call 渲染全部
- WGSL shader 内部通过 flag 区分背景/前景/装饰

理论上 zenterm 在 DOOM-fire 场景下**应该和 tty7 同级别**，甚至更快（因为你的 WGSL shader 可以更精细地控制 glyph type dispatch）。

**建议实测：** 用同样的 DOOM-fire 脚本跑 zenterm，帧率很可能接近 tty7 的 888 fps，差距不会到 2x 级别。

---

## 三、功能对比

### 3.1 zenterm 已有但 tty7 没有的功能

| 功能                   | zenterm                              | tty7                     | 重要性                                   |
| ---------------------- | ------------------------------------ | ------------------------ | ---------------------------------------- |
| **连字 (Ligature)**    | ✅ cosmic-text Shaping::Advanced     | ❌ 不支持                | 高频（Fira Code/Cascadia Code 用户刚需） |
| **像素级完美盒绘字符** | ✅ 软件光栅化 (U+2500-U+259F)        | ❌ 依赖字体路径          | 中高频（`htop`, `tmux`, `border` 场景）  |
| **LCD Subpixel 渲染**  | ✅ swash Format::Subpixel + BGR 检测 | ❌ GPUI 无此层级 API     | 中（文字渲染质量）                       |
| **自定义 WGSL shader** | ✅ 完全可控                          | ❌ GPUI Element API 限制 | 开发灵活性                               |
| **WASM 可行路径**      | ✅ egui + wgpu → WebGPU              | ❌ GPUI 无 WASM          | 长期                                     |
| **模块化架构**         | ✅ 9 crates，独立依赖                | ⚠️ 4 modules，依赖 fork  | 维护性                                   |

### 3.2 tty7 已有但 zenterm 没有的功能

| 功能                              | tty7                    | zenterm              | 实现成本                                |
| --------------------------------- | ----------------------- | -------------------- | --------------------------------------- |
| **Kitty 键盘协议 (CSI u)**        | ✅                      | ❌                   | 低（主要改 input.rs）                   |
| **URL 检测 + 点击打开**           | ✅ (OSC 8 + 裸 URL)     | ❌ (规划中)          | 低（`linkify` crate + 鼠标事件）        |
| **搜索 (Cmd+F)**                  | ✅                      | ❌                   | 中（scrollback 遍历 + overlay UI）      |
| **反向搜索 (Ctrl+R)**             | ✅                      | ❌                   | 中（需要 shell integration 或搜索历史） |
| **内联命令编辑器**                | ✅ (自定义 GPUI 编辑层) | ❌                   | 高（不推荐 copying）                    |
| **命令补全 (~90 spec)**           | ✅ (Fig 格式)           | ❌                   | 高（需要补全引擎 + spec 库）            |
| **语法高亮**                      | ✅                      | ❌                   | 中（tokenizer + 着色）                  |
| **命令历史 (frecency)**           | ✅                      | ❌                   | 中                                      |
| **桌面通知 (OSC 9/777)**          | ✅                      | ❌ (Partially)       | 低（`notify-rust` + OSC handler）       |
| **Shell Integration (OSC 133)**   | ✅ (zsh/bash/fish/pwsh) | ❌                   | 低（注入 rc 文件 + parser）             |
| **Splits (二分树, 可拖动分割线)** | ✅                      | ❌ (未来规划)        | 中高                                    |
| **Command Palette**               | ✅                      | ❌                   | 中                                      |
| **8 套内置主题**                  | ✅                      | ❌ (只有 Dark/Light) | 低（色彩配置）                          |
| **Hot-reload 配置**               | ✅ (notify crate)       | ❌ (需重启)          | 低                                      |
| **设置面板 UI**                   | ✅                      | ⚠️ 部分              | 中                                      |
| **持久化 daemon (tmux-like)**     | ✅                      | ❌                   | 高（见 3.4）                            |

### 3.3 tty7 的亮点功能详解

以下功能值得在 zenterm 中实现（已按优先级排序）：

**P0 — 低成本、高收益：**

1. **Kitty 键盘协议** — 只需修改 `zenterm-input`。VT 查询响应 + 修饰键编码表，约 200 行。让 vim/emacs 获得完整修饰键支持。

2. **URL 检测** — 引入 `linkify` crate（3 行配置），加上 `Ctrl+Click` 事件处理。约 100 行。

3. **桌面通知** — 引入 `notify-rust` crate，OSC 9/777 handler 在 `zenterm-term` 中已有部分代码（`NotificationState` enum 已存在），只需桥接到系统通知。约 150 行。

4. **Shell Integration (OSC 133)** — 注入 zsh/bash 的 rc 文件中，发送 `OSC 133 A`（prompt begin）/ `OSC 133 B`（prompt end）/ `OSC 133 C`（command begin）/ `OSC 133 D`（command end）。用于：
   - 剪切板安全粘贴（只在 prompt 处允许）
   - 长命令完成检测 → 桌面通知
   - 更精确的 cwd 追踪

5. **搜索 (Cmd+F)** — 遍历 scrollback lines，匹配高亮。需要 overlay 输入框 + 导航 controls。约 400 行。

**P1 — 中等成本、有差异化的功能：**

6. **Command Palette** — `Cmd+P` 打开模糊搜索 overlay，搜索动作 + 标签切换。约 300 行。

7. **配置 hot-reload** — 使用 `notify` crate 监听 config 文件变化，diff changeset 后应用。约 200 行。

8. **多套预设主题** — 定义 6-8 套色彩方案，加上用户 override。约 100 行配置。

**P2 — 高成本、可推迟：**

9. **Splits** — 需要改造当前基于 egui_dock 的布局系统，或者在其基础上叠加二分树。

10. **命令补全/编辑器/语法高亮/历史** — tty7 的这套东西是它的核心差异化，但也是最高成本的。建议评估是否与 zenterm 的产品定位匹配。

### 3.4 关于 Daemon 架构

tty7 的 daemon 架构（双进程：`tty7 --daemon` 常驻后台 + GUI 客户端通过 socket 连接）的实际收益：

| 宣称收益                  | 实际价值                                   | 代 价                                     |
| ------------------------- | ------------------------------------------ | ----------------------------------------- |
| GUI 崩溃不丢会话          | 低（现代终端几乎不崩溃）                   | 两个进程 + IPC 协议 + 状态同步 + 连接管理 |
| 重启 GUI 保持会话         | 极低（桌面终端用户关窗口=关会话）          | 同上                                      |
| 支持远程 attach           | 有（但 tty7 目前没有远程网络层）           | 同上                                      |
| 性能（独立进程不阻塞 UI） | 低（单进程时代价也只是 background thread） | 同上                                      |

结论：daemon 架构对于**本地桌面终端**来说，定位非常窄。它的真正价值在远程/服务器场景（SSH attach），但那需要额外的网络层和认证，tty7 目前也没有。**单进程架构对于桌面终端是合理选择。**

---

## 四、zenterm 的差异化定位

1. **渲染管道完全可控** — wgpu + WGSL shader 在手，可以随时添加 GPU 特效、自定义混合、subpixel rendering。tty7 被 GPUI Element API 限制，无法触及这个层面。

2. **连字 + 盒绘渲染质量** — 这是终端日常使用中最显性的视觉差异。tty7 对 Fira Code 用户和 `htop`/`tmux` 用户不友好。

3. **模块化架构** — 9 个独立 crate，各自有清晰的接口边界。这意味着：
   - 可以独立升级任何一个组件（egui、wgpu、alacritty_terminal）
   - 可以独立测试和 bench 每个子系统
   - 可以替换某个 crate 而不影响其他（例如，将来可以把终端渲染抽离出来给其他项目用）

4. **WASM 可行** — egui + wgpu → WebGPU 是天然路径。对于需要浏览器终端的需求，这是实打实的优势。

---

## 五、总结

| 判断                       | 结论                                                                                                                |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| GPUI 是否值得迁移          | **否** — 收益（UI 漂亮）不及成本（重写渲染、功能倒退、依赖不稳定）                                                  |
| egui 是否拖累性能          | **否** — 终端渲染已绕过 egui，瓶颈在 parser 和 grid traversal，与 UI 框架无关                                       |
| tty7 的 2x 性能是否真实    | **部分真实** — DOOM-fire 场景有优势（单 Pass vs Alacritty 双 Pass），但 zenterm 同样采用单 Pass 方案，差距不会到 2x |
| daemon 架构是否值得做      | **否** — 对于桌面终端，收益有限，成本高                                                                             |
| tty7 有哪些值得借鉴        | Kitty 键盘协议、URL 检测、搜索、shell integration、hot-reload 配置                                                  |
| zenterm 有继续维护的价值吗 | **有** — 渲染质量（连字、盒绘、subpixel）、管道可控性、WASM 路径、模块化架构均为真实差异化                          |
