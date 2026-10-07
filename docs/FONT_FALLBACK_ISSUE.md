# 字体回退与渲染问题分析记录 (Font Fallback & Rendering Analysis)

## 1. 现象描述

在终端文本渲染中，目前存在两个关键的字形视觉问题：

1. **单格字形横向溢出裁切（Nerd Font Overhang Clipping）**：
   部分单格字形（如 Nerd Font 图标、特定符号）在设计上带有向右的自然溢出（overhang）。当前渲染管线中执行了严格的基于 cell 边界的 CPU 端裁剪（`clipped_right = glyph_right_px.min(cell_right)`），导致图标右侧笔画被硬性截断。在 WezTerm 等终端中，当字形后继单元格为空格时，允许单格字形向右溢出（即 `WhenFollowedBySpace` 策略）。
2. **CJK 汉字粗体 Fallback 异变与跳跃（Han Bold Fallback Inconsistency）**：
   在常规字重（Normal / 400）下，常用汉字能正确匹配到系统默认字体；但在粗体（Bold / 700）下，相邻汉字会发生混乱的字体跳跃。例如：
   * `构`、`询` 解析为宋体（`Songti SC`，衬线体）；
   * `建` 解析为韩文字体（`Apple SD Gothic Neo`）；
   * 同一行中文混杂了黑体、宋体和韩文字形，且各字符字面率与基线不一致。

---

## 2. 根本原因剖析

### 2.1 `cosmic-text` 的字重严格匹配缺陷

zenterm 采用纯 Rust 字体栈（`cosmic-text + fontdb + swash`）。在排查 `cosmic-text` 0.19.0 的脚本回退流程（`FontFallbackIter`）时，确认了根本原因：

1. **系统字体的字重分布现实**：
   许多操作系统的默认 CJK 字体并不提供标准的 700 面。例如 macOS 的苹方（`PingFang SC`）面分布为：
   * Ultralight (100), Thin (200), Light (300), Regular (400), Medium (500), Semibold (600)。
   * **最高字重为 600，不存在 700 (Bold) 面**。在系统原生排版中，700 会自动映射至 600。
2. **`cosmic-text` 的硬伤逻辑**：
   在 `src/font/fallback/mod.rs` 的非等宽脚本回退匹配中：
   ```rust
   let font_match_keys_iter = |is_mono| {
       self.font_match_keys.iter().filter(move |m_key| {
           m_key.font_weight_diff == 0 || m_key.variable_weight_match || is_mono
       })
   };
   ```
   它强制要求 `m_key.font_weight_diff == 0`（请求字重与面字重绝对误差为 0）。
   当终端请求粗体 700 时，苹方（600）的 `font_weight_diff` 为 100，**直接被脚本回退判定为不匹配并剔除**。
3. **退化为全系统乱搜**：
   在脚本回退失败后，`cosmic-text` 退化为在全系统已装字体中遍历查找 `font_weight_diff == 0` 的任意字体。
   系统内只要有其他字族碰巧带有 700 面且包含该字符编码（例如 Songti SC Bold、Apple SD Gothic Neo Bold），就会被强行采纳，导致汉字在粗体下发生随机的跨字体跳跃。

---

## 3. 失败的应对方案与教训

在之前的排查和试错过程中，曾出现以下反模式方案，在此明确记录以避免再次发生：

1. **单字 Codepoint 范围硬编码（`is_han_character`）**：
   试图硬编码 Unicode 汉字范围，在光栅化阶段对单字创建临时 `Buffer` 探测字体名称。
   * **问题**：破坏排版上下文；无法覆盖日文假名、韩文、阿拉伯文等其他文字系统；增加单字排版开销。
2. **在业务代码中硬编码平台字体名称**：
   在 Rust 代码中按 `cfg(target_os)` 写死 `PingFang SC`、`Microsoft YaHei`、`Noto Sans CJK` 等字面量。
   * **问题**：违背终端跨平台设计；剥夺用户系统语言偏好选择（如繁体中文、日文系统的默认字形）；在字体未安装的 Linux 等系统上脆弱失效。
3. **将回退职责转嫁给用户配置**：
   试图通过要求用户在 `config.toml` 中手动配置 fallback 链来规避底层解析问题。
   * **问题**：违背终端模拟器“开箱即用”的基本质量底线。

---

## 4. 主流终端（WezTerm / Alacritty）的标准实现参考

主流终端在零配置下均能稳定处理回退与粗体降级，核心依赖两条通用规范：

### 4.1 文字系统级绑定（Script-Level Binding）
终端对缺失字符不进行单字逐一猜谜，而是基于 **Unicode Script**（如 `Script::Han`）和当前环境的 **Locale**：
* 首次遇到主字体不支持的 Script 时，定位当前系统针对该文字系统的首选字族；
* **整段文字乃至该文字系统下的后续字符，稳定绑定到该字体族**，确保字形风格、字面大小与笔画特征的一致性。

### 4.2 严格遵循 CSS 字体匹配规范（Family-First Weight Clamping）
根据 CSS Font Matching Specification 的字重匹配规则：
* **字族优先**：一旦确定了文字系统由字族 `F` 承担，样式匹配必须在该字族内部闭环；
* **就近降级**：请求 700 时，若该字族最高仅有 600，**必须就近降级至 600**，或者由光栅器应用算法伪粗体（Synthetic Bold）；
* **严禁越界**：绝对不允许因字重不相等而抛弃该字族、跑到全系统抓取无关字体。

---

## 5. 后续建议重构方向

未来在彻底修复该问题时，建议遵循以下跨平台、零配置的重构路径：

1. **水平溢出裁切策略解耦**：
   针对 MASK 类型的单格字形，实现 `WhenFollowedBySpace` 逻辑：当后继单元格为空格时允许水平溢出绘制，后继有文字或非空格时保留水平裁剪。垂直方向维持原有固定行高裁切。
2. **文字系统级回退调度器**：
   在 `zenterm-glyph` 中抽象一个通用的文字系统回退解析层：
   * 字符缺失时，识别其 Unicode Script；
   * 在启动时已加载的系统字体（`fontdb`）中，根据 Script 覆盖范围和系统 Locale，动态选定该文字系统的承载字族；
   * 该字族确立后，后续样式（Bold / Italic）统一通过 `fontdb` 的标准 CSS 匹配查询具体的 `fontdb::ID`，使粗体自然降级至 Semibold 或对应粗面，根治跨字体跳变。
