//! 多行输入框组件：Enter 换行、上下方向键跨行、按行点击定位。
//!
//! 基础多行语义（硬换行，不做软折行——超宽行裁切显示）：
//! 光标仍是 Field 的平铺 char 下标；行导航 = 找行边界 + 同列近似。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{div, prelude::*, px, App, Bounds, Context, Div, FocusHandle, IntoElement,
           Pixels, Render, SharedString, Styled, Window};

use crate::field::{field_key, Field, FieldResult};
use crate::WellStyle;

type WindowCb = Box<dyn Fn(&mut Window, &mut App)>;

/// 多行输入框（Entity 组件）。
pub struct TextArea {
    pub(crate) field: Field,
    focus_handle: FocusHandle,
    placeholder: SharedString,
    style: std::rc::Rc<WellStyle>,
    blink: bool,
    /// 井内文本原点（首行行首）。
    origin: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 行高（font_size × 1.6，与渲染一致）。
    line_height: f32,
    on_escape: Option<WindowCb>,
}

impl TextArea {
    /// 创建：占位文案 + 外观（height 是井总高，行高按字号推导）。
    pub fn new(cx: &mut Context<Self>, placeholder: &str, style: WellStyle) -> Self {
        let line_height = style.font_size * 1.6;
        let this = Self {
            field: Field::default(),
            focus_handle: cx.focus_handle(),
            placeholder: SharedString::from(placeholder),
            style: std::rc::Rc::new(style),
            blink: true,
            origin: Rc::new(Cell::new(None)),
            line_height,
            on_escape: None,
        };
        this.start_blink(cx);
        this
    }

    /// 绑定 Esc（默认收敛选区）。
    pub fn on_escape(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_escape = Some(Box::new(f));
        self
    }

    pub fn text(&self) -> &str {
        self.field.as_str()
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.field.set(text);
        cx.notify();
    }

    /// 换肤（亮暗主题切换）。
    pub fn restyle(&mut self, style: WellStyle, cx: &mut Context<Self>) {
        self.style = std::rc::Rc::new(style);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.field.clear();
        cx.notify();
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn start_blink(&self, cx: &Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.blink = !this.blink;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    /// 行结构：每行的 (char 起始下标, 行文本)。
    fn lines(&self) -> Vec<(usize, &str)> {
        let text = self.field.as_str();
        let mut out = Vec::new();
        let mut start = 0usize; // char 下标
        let chars = text.char_indices().peekable();
        let mut line_begin_byte = 0usize;
        for (byte, c) in chars {
            if c == '\n' {
                out.push((start, &text[line_begin_byte..byte]));
                start += 1; // 跳过换行符本身
                line_begin_byte = byte + 1;
            }
        }
        out.push((start, &text[line_begin_byte..]));
        out
    }

    /// 光标所在行与行内列。
    fn cursor_line_col(&self) -> (usize, usize) {
        let lines = self.lines();
        let cursor = self.field.cursor;
        for (i, (start, body)) in lines.iter().enumerate() {
            if cursor <= start + body.chars().count() {
                return (i, cursor - start);
            }
        }
        (lines.len() - 1, 0)
    }

    /// 行导航：目标行同列（列钳到目标行长度）。
    fn move_vertically(&mut self, down: bool) {
        let lines = self.lines();
        let (line, col) = self.cursor_line_col();
        let target = if down {
            (line + 1).min(lines.len() - 1)
        } else {
            line.saturating_sub(1)
        };
        if target == line {
            return;
        }
        let (start, body) = lines[target];
        let new_cursor = start + col.min(body.chars().count());
        self.field.collapse_to(new_cursor);
    }

    /// 点击 → 字符下标：先按 y 定行，再按 x 在行内测量。
    fn char_index_at(&self, click: gpui::Point<Pixels>, window: &Window) -> Option<usize> {
        let origin = self.origin.get()?;
        let lines = self.lines();
        let dy = (click.y - origin.origin.y).as_f32();
        let line = (dy / self.line_height).floor().max(0.0) as usize;
        let line = line.min(lines.len() - 1);
        let (start, body) = lines[line];
        if body.is_empty() {
            return Some(start);
        }
        let x = click.x - origin.origin.x;
        let style = window.text_style();
        let run = gpui::TextRun {
            len: body.len(),
            font: gpui::Font {
                family: style.font_family.clone(),
                ..Default::default()
            },
            color: gpui::Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: None,
        };
        let layout = window
            .text_system()
            .layout_line(body, px(self.style.font_size), &[run], None);
        let mut byte = layout.index_for_x(x)?.min(body.len());
        while byte > 0 && !body.is_char_boundary(byte) {
            byte -= 1;
        }
        Some(start + body[..byte].chars().count())
    }

    /// 多行键处理：在 field_key 语义上叠加 行导航 与 换行插入。
    fn dispatch_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) {
        let mods = &ks.modifiers;
        let primary = mods.platform || mods.control;
        // 多行场景的剪贴板组合（含 Ctrl+A 全选；Ctrl+Enter 也可由宿主另行绑定）
        if primary && !mods.alt && !mods.shift {
            match ks.key.as_str() {
                "a" => {
                    self.field.select_all();
                    cx.notify();
                    return;
                }
                "c" => {
                    if let Some(sel) = self.field.selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(sel.to_string()));
                    }
                    return;
                }
                "x" => {
                    if let Some(sel) = self.field.selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(sel.to_string()));
                        self.field.delete_selection();
                        cx.notify();
                    }
                    return;
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                        // 多行：保留换行，仅去 \r
                        let cleaned: String =
                            text.chars().filter(|c| *c != '\r').collect();
                        if self.field.paste_multiline(&cleaned) {
                            cx.notify();
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        if mods.control || mods.alt || mods.platform {
            return;
        }
        match ks.key.as_str() {
            "up" => {
                self.field.collapse_selection();
                self.move_vertically(false);
                cx.notify();
            }
            "down" => {
                self.field.collapse_selection();
                self.move_vertically(true);
                cx.notify();
            }
            "enter" => {
                // 多行语义：Enter 插入换行（选区被替换）
                self.field.insert('\n');
                cx.notify();
            }
            _ => {
                if let FieldResult::Changed = field_key(&mut self.field, ks) {
                    cx.notify();
                }
            }
        }
    }

    /// 渲染：逐行 × 分段交集（选区跨行时各行高亮自己的部分）；
    /// 光标画在 cursor 所在行的段边界上。
    fn content(&self, focused: bool) -> Div {
        let s = &self.style;
        let blink = focused && self.blink;
        let (segs, _caret_at) = self.field.render_segments();
        // 各段的全局 char 边界：[b0=0, b1, b2, b3=总长]
        let mut bounds = Vec::with_capacity(segs.len() + 1);
        bounds.push(0);
        let mut acc = 0;
        for seg in &segs {
            acc += seg.chars().count();
            bounds.push(acc);
        }
        let total = acc;
        let show_sel = focused && self.field.has_selection();
        let cursor = self.field.cursor;

        let mut col = div().flex().flex_col().items_start().gap(px(2.0));
        let lines = self.lines();
        for (li, (lo, line)) in lines.iter().enumerate() {
            let hi = lo + line.chars().count();
            let mut row = div().flex().flex_row().items_center().gap(px(1.0));
            for (seg_idx, _seg) in segs.iter().enumerate() {
                let (s_lo, s_hi) = (bounds[seg_idx], bounds[seg_idx + 1]);
                let a = s_lo.max(*lo);
                let b = s_hi.min(hi);
                // 光标在此段起点 → 画在段前
                if cursor == s_lo && *lo <= cursor && cursor <= hi {
                    row = row.child(caret(s.caret, blink));
                }
                if a < b {
                    let part: String = line.chars().skip(a - lo).take(b - a).collect();
                    row = row.child(
                        div()
                            .text_size(px(s.font_size))
                            .text_color(s.text)
                            .bg(if show_sel && seg_idx == 1 {
                                s.selection
                            } else {
                                gpui::rgba(0x000000_00)
                            })
                            .rounded(px(2.0))
                            .child(SharedString::from(part)),
                    );
                }
            }
            // 光标在全文末尾且这是最后一行 → 画在行尾
            if cursor == total && li == lines.len() - 1 {
                row = row.child(caret(s.caret, blink));
            }
            // 空行也要有行高（无内容时给一个零宽占位）
            if line.is_empty() && !(cursor == total && li == lines.len() - 1) {
                row = row.child(div().w(px(0.0)).h(px(s.font_size)));
            }
            col = col.child(row);
        }
        col
    }
}

impl Render for TextArea {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.style;
        let focused = self.focus_handle.is_focused(_window);
        let origin = self.origin.clone();
        let field_empty = self.field.is_empty();
        let placeholder = self.placeholder.clone();

        div()
            .id("text-area")
            .track_focus(&self.focus_handle)
            .h(px(s.height))
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(10.0))
            .bg(s.bg)
            .border_1()
            .border_color(if focused { s.border_focused } else { s.border })
            .shadow(if focused {
                vec![s.shadow.clone(), s.focus_ring.clone()]
            } else {
                vec![s.shadow.clone()]
            })
            .flex()
            .items_start()
            .overflow_y_scroll()
            .child(gpui::canvas(
                move |b, _, _| origin.set(Some(b)),
                |_, _, _, _| {},
            ))
            .cursor_pointer()
            .on_click(cx.listener(
                |this, ev: &gpui::ClickEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    if let gpui::ClickEvent::Mouse(mouse) = ev {
                        let pos = mouse.up.position;
                        if let Some(idx) = this.char_index_at(pos, window) {
                            if mouse.up.click_count >= 3 {
                                this.field.select_all();
                            } else if mouse.up.click_count == 2 {
                                this.field.select_word(idx);
                            } else {
                                this.field.collapse_to(idx);
                            }
                        }
                    }
                    cx.notify();
                },
            ))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _window, cx| {
                let ks = &ev.keystroke;
                if ks.modifiers.platform {
                    return;
                }
                if ks.key == "escape" {
                    if this.field.has_selection() {
                        let c = this.field.cursor;
                        this.field.collapse_to(c);
                        cx.notify();
                    } else if let Some(f) = &this.on_escape {
                        f(_window, cx);
                    }
                    return;
                }
                this.dispatch_key(ks, cx);
            }))
            .child(if field_empty {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(caret(s.caret, focused && self.blink))
                    .child(
                        div()
                            .text_size(px(s.font_size))
                            .text_color(s.muted)
                            .child(placeholder),
                    )
            } else {
                div().flex_1().min_w(px(0.0)).child(self.content(focused))
            })
    }
}

/// 光标竖条（与 TextInput 同款）。
fn caret(color: gpui::Rgba, visible: bool) -> Div {
    if visible {
        div()
            .w(px(2.0))
            .h(px(14.0))
            .rounded(px(1.0))
            .bg(color)
    } else {
        div().w(px(0.0)).h(px(0.0))
    }
}
