//! 单行输入框组件（`masked()` 即密码框）。
//!
//! 组件自持：编辑键（方向/退格/删除/Home/End）、选区（Shift 扩展、Cmd/Ctrl+A、
//! 双击选词、三击全选）、剪贴板（复制/剪切/粘贴）、点击定位（canvas 记原点 +
//! 文本测量换算）、500ms 光标闪烁。控制字符（回车/Tab 等）不进缓冲。
//! Enter/Tab/Esc 不在此消化，经回调交宿主编排（默认：Enter 聚焦失灵不动作）。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{div, prelude::*, px, App, Bounds, Context, Div, FocusHandle, IntoElement,
           Pixels, Render, SharedString, Styled, Window};

use crate::field::{field_key, ClipOp, Field, FieldResult};
use crate::WellStyle;

type WindowCb = Box<dyn Fn(&mut Window, &mut App)>;
type Decorator =
    Box<dyn Fn(&mut Context<TextInput>, &Field, &WellStyle) -> gpui::AnyElement>;

/// 单行输入框（Entity 组件）。
pub struct TextInput {
    /// 编辑缓冲（公开只读用 `text()`；改动走 `set_text`）。
    pub(crate) field: Field,
    focus_handle: FocusHandle,
    placeholder: SharedString,
    masked: bool,
    style: std::rc::Rc<WellStyle>,
    /// 光标闪烁相位（自有 500ms 定时器翻转）。
    blink: bool,
    /// 井内文本原点（零尺寸 canvas 每帧记录；点击定位用）。
    origin: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Enter 回调（宿主编排：提交/保存/下一个）。
    on_enter: Option<WindowCb>,
    /// Tab 回调（默认聚焦失灵：宿主通常接「下一字段」）。
    on_tab: Option<WindowCb>,
    /// Esc 回调（默认收敛选区）。
    on_escape: Option<WindowCb>,
    /// 前置装饰工厂（每帧现造；参数带自家缓冲，**禁止在工厂里重读自身实体**——
    /// 渲染期间实体处于可变借用，Entity::read(self) 会 BorrowMut panic）。
    leading: Option<Decorator>,
    /// 后置装饰工厂（同上；参数含当前样式，主题切换即时生效）。
    trailing: Option<Decorator>,
}

impl TextInput {
    /// 创建：占位文案 + 外观。
    pub fn new(cx: &mut Context<Self>, placeholder: &str, style: WellStyle) -> Self {
        let this = Self {
            field: Field::default(),
            focus_handle: cx.focus_handle(),
            placeholder: SharedString::from(placeholder),
            masked: false,
            style: std::rc::Rc::new(style),
            blink: true,
            origin: Rc::new(Cell::new(None)),
            on_enter: None,
            on_tab: None,
            on_escape: None,
            leading: None,
            trailing: None,
        };
        this.start_blink(cx);
        this
    }

    /// 密码框：圆点遮蔽（选区/光标语义保留）。
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// 绑定 Enter（提交类动作）。
    pub fn on_enter(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_enter = Some(Box::new(f));
        self
    }

    /// 绑定 Tab（通常是「聚焦下一字段」）。
    pub fn on_tab(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_tab = Some(Box::new(f));
        self
    }

    /// 绑定 Esc。
    pub fn on_escape(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_escape = Some(Box::new(f));
        self
    }

    /// 前置装饰工厂（每帧现造元素；参数 = 组件 Context + 自家编辑缓冲）。
    pub fn leading(
        mut self,
        f: impl Fn(&mut Context<Self>, &Field, &WellStyle) -> gpui::AnyElement + 'static,
    ) -> Self {
        self.leading = Some(Box::new(f));
        self
    }

    /// 后置装饰工厂（参数 = 组件 Context + 自家编辑缓冲）。
    pub fn trailing(
        mut self,
        f: impl Fn(&mut Context<Self>, &Field, &WellStyle) -> gpui::AnyElement + 'static,
    ) -> Self {
        self.trailing = Some(Box::new(f));
        self
    }

    /// 建后接线：Enter。
    pub fn set_on_enter(&mut self, f: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_enter = Some(Box::new(f));
    }

    /// 建后接线：Tab。
    pub fn set_on_tab(&mut self, f: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_tab = Some(Box::new(f));
    }

    /// 建后接线：Esc。
    pub fn set_on_escape(&mut self, f: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_escape = Some(Box::new(f));
    }

    /// 切换掩码（眼睛按钮：明文/圆点）。
    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    /// 当前文本。
    pub fn text(&self) -> &str {
        self.field.as_str()
    }

    /// 整体设值（光标归位末尾）。
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.field.set(text);
        cx.notify();
    }

    /// 换肤（亮暗主题切换）。
    pub fn restyle(&mut self, style: WellStyle, cx: &mut Context<Self>) {
        self.style = std::rc::Rc::new(style);
        cx.notify();
    }

    /// 清空。
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.field.clear();
        cx.notify();
    }

    /// 焦点句柄（宿主聚焦/编排 Tab 用）。
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// 光标闪烁定时器（每 500ms 翻转相位；实体释放即停）。
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

    /// field_key 结果分发（含剪贴板三操作）。
    fn dispatch_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) {
        match field_key(&mut self.field, ks) {
            FieldResult::Changed => cx.notify(),
            FieldResult::Unchanged => {}
            FieldResult::Clipboard(ClipOp::Paste) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    if self.field.paste(&text) {
                        cx.notify();
                    }
                }
            }
            FieldResult::Clipboard(ClipOp::Copy) => {
                if let Some(sel) = self.field.selected_text() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(sel.to_string()));
                }
            }
            FieldResult::Clipboard(ClipOp::Cut) => {
                if let Some(sel) = self.field.selected_text() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(sel.to_string()));
                    self.field.delete_selection();
                    cx.notify();
                }
            }
        }
    }

    /// 点击 → 字符下标（原点 + 文本测量；字节下标收敛字符边界）。
    fn char_index_at(&self, click_x: Pixels, window: &Window) -> Option<usize> {
        let origin = self.origin.get()?;
        let text = self.field.as_str();
        let x = click_x - origin.origin.x;
        if text.is_empty() {
            return Some(0);
        }
        let style = window.text_style();
        let run = gpui::TextRun {
            len: text.len(),
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
            .layout_line(text, px(self.style.font_size), &[run], None);
        let mut byte = layout.index_for_x(x)?.min(text.len());
        while byte > 0 && !text.is_char_boundary(byte) {
            byte -= 1;
        }
        Some(text[..byte].chars().count())
    }

    /// 内容行：选区三段高亮 + 光标画在活动端；掩码时按段切圆点。
    fn content_row(&self, focused: bool) -> Div {
        let t = &self.style;
        let blink = focused && self.blink;
        let (segs, caret_at) = self.field.render_segments();
        let show_sel = focused && self.field.has_selection();
        let mut row = div().flex().flex_row().items_center().gap(px(1.0));
        for (i, seg) in segs.iter().enumerate() {
            if i == caret_at {
                row = row.child(caret(t.caret, blink));
            }
            let body = if self.masked {
                "•".repeat(seg.chars().count())
            } else {
                seg.to_string()
            };
            row = row.child(
                div()
                    .text_size(px(t.font_size))
                    .text_color(t.text)
                    .bg(if show_sel && i == 1 { t.selection } else { gpui::rgba(0x000000_00) })
                    .rounded(px(2.0))
                    .child(SharedString::from(body)),
            );
        }
        if caret_at >= segs.len() {
            row = row.child(caret(t.caret, blink));
        }
        row
    }
}

/// 光标竖条（未显示时零宽不占位）。
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

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.style;
        let focused = self.focus_handle.is_focused(_window);
        let origin = self.origin.clone();
        let field_empty = self.field.is_empty();
        let placeholder = self.placeholder.clone();

        div()
            .id("text-input")
            .track_focus(&self.focus_handle)
            .h(px(s.height))
            .px(px(12.0))
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
            .items_center()
            .gap(px(8.0))
            // 零尺寸 canvas：每帧记录井内文本原点（点击定位）
            .child(gpui::canvas(
                move |b, _, _| origin.set(Some(b)),
                |_, _, _, _| {},
            ))
            .children(self.leading.as_ref().map(|f| f(cx, &self.field, &self.style)))
            .cursor_pointer()
            .on_click(cx.listener(
                |this, ev: &gpui::ClickEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    if let gpui::ClickEvent::Mouse(mouse) = ev {
                        let x = mouse.up.position.x;
                        let count = mouse.up.click_count;
                        if let Some(idx) = this.char_index_at(x, window) {
                            if count >= 3 {
                                this.field.select_all();
                            } else if count == 2 {
                                this.field.select_word(idx);
                            } else {
                                this.field.collapse_to(idx);
                            }
                        }
                    }
                    cx.notify();
                },
            ))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                let ks = &ev.keystroke;
                if ks.modifiers.platform {
                    return;
                }
                match ks.key.as_str() {
                    "enter" => {
                        if let Some(f) = &this.on_enter {
                            f(window, cx);
                        }
                    }
                    "tab" => {
                        if let Some(f) = &this.on_tab {
                            f(window, cx);
                        }
                    }
                    "escape" => {
                        if this.field.has_selection() {
                            let c = this.field.cursor;
                            this.field.collapse_to(c);
                            cx.notify();
                        } else if let Some(f) = &this.on_escape {
                            f(window, cx);
                        }
                    }
                    _ => this.dispatch_key(ks, cx),
                }
            }))
            .children(self.trailing.as_ref().map(|f| f(cx, &self.field, &self.style)))
            .child(if field_empty {
                // 空态：光标在最左，占位灰字随后（原生输入框行为）
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
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .child(self.content_row(focused))
            })
    }
}
