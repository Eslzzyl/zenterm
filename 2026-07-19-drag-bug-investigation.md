# 2026-07-19 标签拖拽 Bug 调查记录

## Bug 现象

拖拽标签时，出现两个标签按钮跟随光标移动。一个移动速度与光标一致，另一个移动速度为光标的约两倍，且随着拖拽距离增加，两个按钮间距逐渐增大。

## 前置排查

### 独立复现失败

用 `egui_dock 0.19.1` + `egui 0.34.3` 创建了独立的复现项目，配置最简单的 `TabViewer`（`String` 类型 tab，只显示 label 和空内容）。拖拽行为完全正常，无法复现 bug。

结论：bug 不单纯是 egui_dock + egui 的版本组合问题。

### wgpu 回调排除

注释掉 `dock.rs` 中的 `egui_wgpu::Callback::new_paint_callback()` 调用，禁用所有 GPU cell 渲染，仅保留 egui 侧绘制。Bug 仍然存在。

结论：bug 不在 wgpu 回调 / GPU 渲染路径中。

## 在 egui_dock 中的诊断

以下诊断通过在 patched egui_dock 中添加 `log::warn!` 级别日志完成。

### 诊断 1：`transform_layer_shapes` 调用次数

在 `leaf.rs` 中 `transform_layer_shapes` 前后加日志，记录每次调用的 `delta`、`layer_id`、指针位置、以及 `to_global` 持久变换检查。

结果（56 个拖拽帧的数据）：

- 每帧恰好调用 **1 次**
- `delta` 值随指针平滑增长，无跳变
- `shapes_in_layer_before` 值无异常
- `[DOCK] PERSISTENT to_global transform ACTIVE` 从未触发

### 诊断 2：`show_leaf` 调用频率

在 `leaf.rs` 的 `show_leaf` 入口加每帧计数器（帧内重置）。

结果：

- 每帧 `show_leaf` 被调用 **1 次**
- leaf 路径始终为 `(SurfaceIndex(0), NodeIndex(0))`
- 树结构 `total_nodes=1, actual_leaves=1`

### 诊断 3：`render_nodes` 调用频率

在 `mod.rs` 的 `render_nodes` 入口加每帧计数器（帧内重置）。

结果：

- 每帧调用 `render_nodes` **1 次**，surface 固定为 `SurfaceIndex(0)`
- leaf 循环迭代 **1 次**，`node_indices=[NodeIndex(0)]`

### 诊断 4：`show_surface_inside` 分支

在 `show_surface_inside` 中记录走了哪个分支。

结果：

- 每帧走 `show_root_surface_inside` 路径 **1 次**
- 从不走 `show_window_surface` 路径
- 表面数（`valid_surface_indices().len()`）始终为 **1**

### 诊断 5：`tab_title` 绘制层检查

在 `tab_title` 中，当 `is_being_dragged=true` 时记录 `ui.layer_id()`。

结果：

- `[DOCK] tab_title painting in WRONG layer!` 从未触发
- 绘制层正确为 `Tooltip` 层

### 诊断 6：`to_global` 持久变换检查

在帧结尾检查拖拽标签对应 `layer_id` 在 `ctx.layer_transform_to_global()` 中的返回值。

结果：

- 从未发现 `to_global` 中有该 `layer_id` 的任何条目

## 在 zenterm 中的诊断

### `TabViewer::ui` 调用日志

在 `tab_viewer.rs` 的 `TabViewer::ui` 中记录每次调用的参数。

结果：

- 同一个 session 在同一个处理周期中被连续调用 **多次**（2-9 次）
- 但所有调用的参数完全一致：
  - `max_rect=[[220.0 30.0] - [724.0 436.0]]`
  - `origin_px=(440,60)`
  - `dock_vp=(440,0)-(1008,872)`
- 拖拽中这些值稳定不变，无跳变

这个模式可能是 `tab_body` 中的 `ScrollArea` 或 `Frame::show` 导致的重复渲染，不影响视觉效果。

### `show_active_indicator` 边框

`show_active_indicator` 在有多 leaf（拆分视图）时在活动 tab 周围画一个彩色矩形边框。单 leaf 时被设为 `false` 禁用。本次测试中均为单 leaf，该边框未绘制。

## 已排除的因素列表

| 因素                              | 排除依据                         |
| --------------------------------- | -------------------------------- |
| `transform_layer_shapes` 被调多次 | 日志确认每帧 1 次                |
| `to_global` 有残留持久变换        | 日志确认不存在                   |
| `show_leaf` 被调多次              | 修正后的计数器确认每帧 1 次      |
| `render_nodes` 被调多次           | 每帧 1 次                        |
| 树中有多个 leaf                   | `total_nodes=1, actual_leaves=1` |
| `tab_title` 画到了错误层          | 日志确认正确为 Tooltip 层        |
| wgpu 回调渲染位置错误             | 禁用回调后 bug 仍在              |
| session viewport 坐标在拖拽中出错 | 日志显示稳定不变                 |
| 独立复现（无 zenterm 代码）       | 无法复现                         |
| egui 版本                         | 复现项目同版本无问题             |
| egui_dock 版本                    | 复现项目同版本无问题             |
