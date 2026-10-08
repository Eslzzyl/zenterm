# Glyph Quad Clipping

## 问题

swash 光栅化 glyph 时，位图的包围盒（`placement.top` + `placement.height`）
可能超出 cell 边界。原因：

1. **包围盒基于矢量轮廓**：swash 遍历所有轮廓点（包括 Bézier 控制点），
   控制点可以超出实际曲线，导致包围盒偏大
2. **像素对齐的 floor/ceil**：包围盒对齐到像素网格时，上下各可能多出 1px
3. **OS/2 度量与实际轮廓不一致**：cosmic-text 的 `max_descent` 来自字体的
   OS/2 表（排版建议值），而 swash 的位图高度来自实际轮廓像素范围。
   个别字符的轮廓可以超出排版度量

这条风险描述适用于旧版 shader：透明像素会以 `bg_color` 写入 framebuffer。
当前 MASK shader 使用 coverage alpha，coverage 为 0 时输出透明像素。
因此 MASK 字形可以在后继 cell 为空时保留必要的横向 overhang。

## 行高与 fallback

终端网格采用配置主字体的固定 cell 尺寸。`GlyphAtlas::measure_baseline` 只用
主字体的 `Mg` 测量 ascent/descent；CJK、日文、韩文等 fallback 字体不会把自己
的度量传播到所有行，否则某个脚本就会把整个终端的行高撑大。

`cosmic-text::LayoutGlyph.font_id` 会保留每个 glyph 实际选中的字体。swash 光栅化
后，如果该 fallback glyph 的 bitmap 包围盒超出主字体 cell，就先按安全比例降低
font size 并重新光栅化，以 baseline 为中心保持 bearing 关系；重新光栅化后的取整或
hinting 残差才交给渲染时的 scale 处理。缩放只作为异常 fallback 的安全适配，主字体
glyph 保持原始尺寸。fallback glyph
从光栅化阶段就使用灰度 mask，使 fallback glyph 保持稳定的物理采样；
若后续对异常字形执行几何缩放，也不会重新引入 LCD coverage。这样可以保留固定行高，
渲染层继续负责垂直裁切；MASK 的横向处理遵循后继 cell 内容。

Han fallback family 由 `FontResolver` 按 Unicode Script、locale、平台 fallback 列表和字体覆盖情况选择。
同一 atlas 生命周期内，该 Script 保持固定字族绑定，相邻 Han 字符使用同一字族。
请求字重和斜体属性在绑定字族内部匹配；静态字族使用最近字重，可变 `wght` 轴使用请求值。


## 解决方案

渲染层继续处理垂直裁切。横向处理采用 WezTerm 默认的
`WhenFollowedBySpace` 策略，并读取 glyph 自身的 Unicode 宽度：

- MASK 宽字符按 2 个 cell 保留完整 bitmap；
- MASK 单格 glyph 的后继 cell 为空时，保留横向 overhang；
- MASK 单格 glyph 的后继 cell 有内容时，按 cell 范围裁切；
- SUBPIXEL 和 COLOR 继续裁切，直到各自的透明像素与背景合成路径支持 overhang。

代码位于 `crates/zenterm-ui/src/session/render/mod.rs`，非连字 glyph 渲染路径中：

```rust
// 垂直裁剪
let glyph_bot_px = glyph_y_px + scaled_h;
let clipped_top = glyph_y_px.max(cell_top);
let clipped_bot = glyph_bot_px.min(cell_bottom);
let clipped_h = (clipped_bot - clipped_top).max(0.0);
if clipped_h < scaled_h && scaled_h > 0.0 {
    let r_top = (clipped_top - glyph_y_px) / scaled_h;
    let r_bot = (clipped_bot - glyph_y_px) / scaled_h;
    let v_range = v_max - v_min;
    v_min = v_min + v_range * r_top;
    v_max = v_min + (r_bot - r_top) * v_range;
    glyph_y_px = clipped_top;
    scaled_h = clipped_h;
}

// 横向裁切
let allow_horizontal_overflow =
    glyph_type == GlyphContentType::Mask
        && next_cell_is_space;
if !allow_horizontal_overflow {
    let clipped_left = glyph_x_px.max(cell_left);
    let clipped_right = glyph_right_px.min(cell_right);
    // 同步调整 u_min / u_max
}

横向 overhang 只在空 cell 上绘制真实字形像素。MASK 的透明像素 alpha 为 0，
不会改写后继 cell 的背景。

## 宽字符（CJK / Emoji）处理

全角字符占据两个 cell。普通 glyph 优先读取下一列的 `is_spacer`，
同时使用 `UnicodeWidthChar` 校正字符自身的显示宽度：

```rust
let num_cells: f32 = if col + 1 < cols {
    grid.cell(row, col + 1)
        .map_or(1.0, |c| if c.is_spacer { 2.0 } else { 1.0 })
} else {
    1.0
};

let cell_right = cell_left + cw * num_cells;  // 半角: +cw, 全角: +2*cw
```

背景 quad 同样使用 `num_cells` 确保全角字符的背景覆盖完整的两列。

宽字符的后继 cell 不单独生成背景或 glyph instance，无论该 cell 使用
`is_spacer` 还是普通空格标志。首个 cell 负责整段背景与字形，保证
block cursor、selection 背景覆盖完整宽度。

## IME 预编辑

输入法预编辑文字覆盖在终端网格上，底层网格没有为预编辑字符写入
`is_spacer`。渲染层需要单独按 `UnicodeWidthChar` 计算预编辑字符占用的 cell：

- CJK、全角符号和宽 emoji 占 2 个 cell；
- 组合字符采用至少 1 个 cell 的绘制宽度；
- 宽字符的后继 cell 跳过底层背景与 glyph，首个 cell 负责绘制完整 bitmap；
- 视觉游标移动到预编辑文字占用的总 cell 数之后。

这样预编辑中文与已经提交到终端网格的中文采用相同的宽度规则。

## 与其他终端的对比

| | 横向策略 | 透明像素处理 |
|--|-----------|--------------|
| Alacritty | 保留 glyph bitmap 宽度 | dual-source / alpha 路径 |
| WezTerm | `WhenFollowedBySpace` 默认 | 根据渲染模式选择合成路径 |
| zenterm | MASK 跟随 `WhenFollowedBySpace` | MASK 使用 coverage alpha |

zenterm 的垂直裁切继续保护固定行高。MASK 的横向裁切按后继 cell 内容决定，
使 Nerd Font 图标可以保留设计中的横向 overhang，同时让相邻文字保持独立。

## 性能影响

可忽略。裁剪逻辑是每个 glyph 几次浮点比较和加减法，发生在 CPU 端
instance 构建阶段（非热路径）。对于完全在 cell 内的 glyph，
`if` 分支不进入。GPU 端无变化。
