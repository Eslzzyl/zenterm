# 汉字字体回退与粗体跳跃问题分析记录 (Han Fallback & Bold Inconsistency Analysis)

> **状态：已实施。** 通用 Unicode Script 字体回退与字族内字重匹配已接入 `zenterm-glyph`。

## 1. 现象描述

历史版本的终端文本渲染曾出现关键的汉字粗体 Fallback 异变与跳跃问题：

* **CJK 汉字粗体 Fallback 异变与跳跃（Han Bold Fallback Inconsistency）**：
  在常规字重（Normal / 400）下，常用汉字能正确匹配到系统默认字体；此前的粗体（Bold / 700）处理会让相邻汉字发生跨字体跳跃。例如：
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

## 5. 已实施方案

实现位于 `crates/zenterm-glyph/src/font_resolver.rs`，并由
`crates/zenterm-glyph/src/atlas_impl.rs` 的三条终端 shaping 路径统一使用。

### 5.1 Script 与字族解析

`FontResolver` 在 `GlyphAtlas` 创建时建立一次字体索引：

1. 收集每个 `fontdb::FaceInfo` 关联的全部字族名、face ID、字重、样式和等宽属性；
2. 使用 `unicode-script` 按 UTF-8 字节范围切分文本；
3. 对每个强 Script 保持一个 atlas 生命周期内固定的字族绑定；
4. `Common`、`Inherited`、`Unknown` 使用配置的主字族，继续交由 `cosmic-text` 处理符号和组合字符；
5. 字族选择顺序为主字族、`PlatformFallback` 的 locale 列表、数据库中的等宽字族、数据库中的全部字族；
6. 字符覆盖检查使用 `Database::with_face_data`、`ttf_parser::Face::glyph_index` 和 `(fontdb::ID, u32)` 缓存。

### 5.2 字族内字重匹配

选定字族后，解析器按照样式匹配、请求字重能力、字重距离、face 字重进行选择：

- 静态 face 使用字族内最近字重；
- 静态字族提供 400 和 600 时，请求 700 选择 600；
- 字族提供 700 时，请求 700 选择 700；
- `wght` 可变轴覆盖请求值时，返回请求字重，让 `cosmic-text` 实例化对应轴值；
- 原始 `style` 继续传入 shaping 属性，保留 cosmic-text 的样式合成行为；
- 每个新 Script 绑定记录一次 `font fallback binding` 日志，实际 swash face 继续使用现有 `log_font_face` 诊断。

### 5.3 shaping 路径接入

`GlyphAtlas` 的以下路径统一通过 resolved attributes 设置 `Buffer`：

- `baseline_glyph_ids`
- `shape_and_rasterize_run_with_style`
- `rasterize_glyph`

单字族文本使用 `Buffer::set_text`；多字族 UTF-8 字节范围使用
`Buffer::set_rich_text`。基础 `Attrs` 的 style、font features、metadata、metrics、
cache flags 和 decorations 全部保留，family 与 weight 使用解析结果。

终端 cell width、line height、baseline、cursor geometry 和 underline measurement
继续使用配置的主字族。`GlyphCacheKey`、`RunCacheKey`、atlas 重建和现有缓存生命周期
保持原有接口。

### 5.4 测试覆盖

`font_resolver` 测试覆盖静态 400/600 最近字重、静态 400/700 精确字重、
可变 `wght` 轴和样式优先级。系统字体测试使用 `FontSystem::new()` 与 `构询建`，
检查粗体两次解析的字族稳定性以及 regular/bold 的字族一致性。

`atlas_impl` shaping 测试使用真实安装字体创建 `Buffer`，收集
`LayoutGlyph::font_id`，确认 Han glyph 来自解析器绑定的字族。数据库缺少样本文字时，
测试输出平台诊断信息。

已执行：

- `cargo fmt --check`
- `cargo test -p zenterm-glyph`
- `cargo test --workspace`
