//! 单行文本编辑缓冲：自绘输入井的编辑模型（文本 + 光标 + 选区）。
//!
//! 光标与选区边界一律按 **char 下标**计（0..=len），中文等多字节字符按一个字符处理。
//! 选区 = cursor（活动端，光标所在）与 anchor（锚点）不同的区间；
//! 插入/删除优先作用于选区——与原生单行输入框语义一致。

/// 编辑缓冲：文本 + 光标 + 选区锚点。
#[derive(Default)]
pub struct Field {
    /// 文本内容。
    pub text: String,
    /// 光标位置（选区活动端；char 下标 0..=char_len）。
    pub cursor: usize,
    /// 选区锚点（另一端）；anchor == cursor 表示无选区。
    pub anchor: usize,
}

/// 单行输入的字符数上限（统一值；足够任何字段使用，防异常粘贴撑爆布局）。
const MAX_CHARS: usize = 200;

impl Field {
    /// 整体设值（生成密码、表单回填等），光标归位末尾、清空选区。
    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.char_len();
        self.anchor = self.cursor;
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.anchor = 0;
    }

    /// 字符数（不是字节数）。
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// char 下标 → 字节下标（越界取 len）。
    fn byte_index_of(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    /// 渲染切分（无选区）：(光标前, 光标后)。
    pub fn split(&self) -> (&str, &str) {
        let b = self.byte_index_of(self.cursor);
        (&self.text[..b], &self.text[b..])
    }

    /// 渲染分段（含选区）：(段列表, 光标画在第几段之后)。
    /// 无选区两段；有选区三段（中段为选区），光标画在 cursor 活动端。
    pub fn render_segments(&self) -> (Vec<&str>, usize) {
        if self.anchor == self.cursor {
            let b = self.byte_index_of(self.cursor);
            (vec![&self.text[..b], &self.text[b..]], 1)
        } else {
            let (start, end) = self.sel_range().expect("已判有选区");
            let (bs, be) = (self.byte_index_of(start), self.byte_index_of(end));
            let segs = vec![&self.text[..bs], &self.text[bs..be], &self.text[be..]];
            // cursor 在选区尾 → 光标画在选区后（第 2 段之后）；在头 → 画在选区前（第 0 段之后）
            let caret_at = if self.cursor > self.anchor { 2 } else { 1 };
            (segs, caret_at)
        }
    }

    /// 选区范围（char 下标，start < end）；无选区 None。
    pub fn sel_range(&self) -> Option<(usize, usize)> {
        if self.anchor == self.cursor {
            None
        } else {
            Some((self.anchor.min(self.cursor), self.anchor.max(self.cursor)))
        }
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.cursor
    }

    /// 选区文本；无选区 None。
    pub fn selected_text(&self) -> Option<&str> {
        self.sel_range()
            .map(|(a, b)| &self.text[self.byte_index_of(a)..self.byte_index_of(b)])
    }

    /// 全选。
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.char_len();
    }

    /// 收敛光标到指定位置（清空选区）。
    pub fn collapse_to(&mut self, char_idx: usize) {
        self.cursor = char_idx.min(self.char_len());
        self.anchor = self.cursor;
    }

    /// 以 at 为中心选词（字母数字连续段；落在非词字符上仅定位）。
    pub fn select_word(&mut self, at: usize) {
        let chars: Vec<char> = self.text.chars().collect();
        let n = chars.len();
        let at = at.min(n.saturating_sub(1));
        let is_word = |c: char| c.is_alphanumeric();
        if at >= n || !is_word(chars[at]) {
            self.collapse_to(at);
            return;
        }
        let mut start = at;
        while start > 0 && is_word(chars[start - 1]) {
            start -= 1;
        }
        let mut end = at;
        while end < n && is_word(chars[end]) {
            end += 1;
        }
        self.anchor = start;
        self.cursor = end;
    }

    /// 删除选区（无选区则不动）。返回是否有改动。
    pub fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.sel_range() {
            let (ba, bb) = (self.byte_index_of(a), self.byte_index_of(b));
            self.text.replace_range(ba..bb, "");
            self.cursor = a;
            self.anchor = a;
            true
        } else {
            false
        }
    }

    /// 光标处（或替换选区）插入一个字符；达上限静默忽略。
    pub fn insert(&mut self, c: char) {
        self.delete_selection();
        if self.char_len() >= MAX_CHARS {
            return;
        }
        let b = self.byte_index_of(self.cursor);
        self.text.insert(b, c);
        self.cursor += 1;
        self.anchor = self.cursor;
    }

    /// 粘贴：单行化（去换行）、替换选区、容量截断。返回是否有改动。
    pub fn paste(&mut self, text: &str) -> bool {
        let cleaned: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        if cleaned.is_empty() {
            return false;
        }
        self.delete_selection();
        let room = MAX_CHARS.saturating_sub(self.char_len());
        if room == 0 {
            return false;
        }
        let take: String = cleaned.chars().take(room).collect();
        let b = self.byte_index_of(self.cursor);
        self.text.insert_str(b, &take);
        self.cursor += take.chars().count();
        self.anchor = self.cursor;
        true
    }

    /// 多行粘贴：保留换行（只滤 \r），替换选区，容量截断。
    pub fn paste_multiline(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.delete_selection();
        let room = MAX_CHARS.saturating_sub(self.char_len());
        if room == 0 {
            return false;
        }
        let take: String = text.chars().take(room).collect();
        let b = self.byte_index_of(self.cursor);
        self.text.insert_str(b, &take);
        self.cursor += take.chars().count();
        self.anchor = self.cursor;
        true
    }

    /// 把选区收敛到光标（不移动光标）。
    pub fn collapse_selection(&mut self) {
        self.anchor = self.cursor;
    }

    /// 退格：优先删选区，否则删光标前一个字符。
    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor == 0 {
            return false;
        }
        let start = self.byte_index_of(self.cursor - 1);
        let end = self.byte_index_of(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
        self.anchor = self.cursor;
        true
    }

    /// 前向删除：优先删选区，否则删光标处字符。
    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor >= self.char_len() {
            return false;
        }
        let start = self.byte_index_of(self.cursor);
        let end = self.byte_index_of(self.cursor + 1);
        self.text.replace_range(start..end, "");
        true
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.anchor = self.cursor;
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_len());
        self.anchor = self.cursor;
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
        self.anchor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.char_len();
        self.anchor = self.cursor;
    }

    /// Shift+左：光标左移，锚点不动（扩展选区）。
    pub fn extend_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Shift+右：光标右移，锚点不动。
    pub fn extend_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_len());
    }

    /// Shift+Home：光标到头，锚点不动。
    pub fn extend_home(&mut self) {
        self.cursor = 0;
    }

    /// Shift+End：光标到尾，锚点不动。
    pub fn extend_end(&mut self) {
        self.cursor = self.char_len();
    }
}

/// 剪贴板操作（field_key 只发信号，读写由调用方执行）。
#[derive(Debug, PartialEq, Eq)]
pub enum ClipOp {
    Copy,
    Cut,
    Paste,
}

/// field_key 的处理结果。
#[derive(Debug, PartialEq, Eq)]
pub enum FieldResult {
    /// 未识别的键或组合，无改动。
    Unchanged,
    /// 缓冲已更新，需要重绘。
    Changed,
    /// 需要剪贴板操作（调用方读写后继续）。
    Clipboard(ClipOp),
}

/// 统一的编辑键处理。
/// 主修饰键（mac=Cmd / 其他=Ctrl）+ a/c/x/v 走全选与剪贴板；
/// Shift+方向/Home/End 扩展选区；普通方向/退格/删除/字符按光标语义。
pub fn field_key(field: &mut Field, ks: &gpui::Keystroke) -> FieldResult {
    let mods = &ks.modifiers;
    let primary = mods.platform || mods.control;
    if primary && !mods.alt && !mods.shift {
        return match ks.key.as_str() {
            "a" => {
                field.select_all();
                FieldResult::Changed
            }
            "c" => FieldResult::Clipboard(ClipOp::Copy),
            "x" => FieldResult::Clipboard(ClipOp::Cut),
            "v" => FieldResult::Clipboard(ClipOp::Paste),
            _ => FieldResult::Unchanged,
        };
    }
    if mods.control || mods.alt || mods.platform {
        return FieldResult::Unchanged;
    }
    match ks.key.as_str() {
        "left" => {
            if mods.shift {
                field.extend_left();
            } else {
                field.move_left();
            }
            FieldResult::Changed
        }
        "right" => {
            if mods.shift {
                field.extend_right();
            } else {
                field.move_right();
            }
            FieldResult::Changed
        }
        "home" => {
            if mods.shift {
                field.extend_home();
            } else {
                field.move_home();
            }
            FieldResult::Changed
        }
        "end" => {
            if mods.shift {
                field.extend_end();
            } else {
                field.move_end();
            }
            FieldResult::Changed
        }
        "backspace" => {
            if field.backspace() {
                FieldResult::Changed
            } else {
                FieldResult::Unchanged
            }
        }
        "delete" => {
            if field.delete_forward() {
                FieldResult::Changed
            } else {
                FieldResult::Unchanged
            }
        }
        key => {
            // 键入字符优先取 key_char（真实输入字符：Shift 大写、Option 符号）；
            // 回退到单字符 key（且不按 Shift，避免把 shift+a 错录成小写）。
            let typed = ks.key_char.as_deref().and_then(|s| {
                let mut it = s.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) => Some(c),
                    _ => None,
                }
            });
            let fallback = (!mods.shift).then(|| {
                let mut it = key.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) => Some(c),
                    _ => None,
                }
            });
            if let Some(c) = typed.or(fallback.flatten()) {
                // 控制字符（回车与 Tab 的 key_char 是控制码）一概不进单行缓冲
                if c.is_control() {
                    FieldResult::Unchanged
                } else {
                    field.insert(c);
                    FieldResult::Changed
                }
            } else {
                FieldResult::Unchanged
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &str) -> gpui::Keystroke {
        gpui::Keystroke {
            key: key.to_string(),
            modifiers: Default::default(),
            key_char: None,
        }
    }

    fn ks_char(key: &str, key_char: &str) -> gpui::Keystroke {
        gpui::Keystroke {
            key: key.to_string(),
            modifiers: Default::default(),
            key_char: Some(key_char.to_string()),
        }
    }

    /// 中段插入：光标处插字符，后续内容右移。
    #[test]
    fn insert_at_cursor() {
        let mut f = Field::default();
        f.set("ac");
        f.cursor = 1;
        f.anchor = 1;
        assert_eq!(field_key(&mut f, &ks("b")), FieldResult::Changed);
        assert_eq!(f.as_str(), "abc");
        assert_eq!(f.cursor, 2);
    }

    /// 有选区时插入 = 替换选区。
    #[test]
    fn insert_replaces_selection() {
        let mut f = Field::default();
        f.set("hello world");
        f.anchor = 5;
        f.cursor = 10; // 选中 " worl"… 实际 "world" 尾部
        assert_eq!(f.selected_text(), Some(" worl"));
        f.insert('!');
        assert_eq!(f.as_str(), "hello!d");
        assert!(!f.has_selection());
    }

    /// 中段退格与前向删除；选区优先。
    #[test]
    fn backspace_and_delete() {
        let mut f = Field::default();
        f.set("abc");
        f.cursor = 2;
        f.anchor = 2;
        assert_eq!(field_key(&mut f, &ks("backspace")), FieldResult::Changed);
        assert_eq!(f.as_str(), "ac");
        assert_eq!(f.cursor, 1);

        let mut f = Field::default();
        f.set("abc");
        f.cursor = 1;
        f.anchor = 1;
        assert_eq!(field_key(&mut f, &ks("delete")), FieldResult::Changed);
        assert_eq!(f.as_str(), "ac");

        // 选区退格 = 删选区
        let mut f = Field::default();
        f.set("abcd");
        f.anchor = 1;
        f.cursor = 3;
        assert_eq!(field_key(&mut f, &ks("backspace")), FieldResult::Changed);
        assert_eq!(f.as_str(), "ad");
    }

    /// 方向键越界钳制；移动即收敛选区。
    #[test]
    fn arrows_and_bounds() {
        let mut f = Field::default();
        f.set("abc");
        f.move_home();
        f.move_left();
        assert_eq!(f.cursor, 0);
        f.move_end();
        f.move_right();
        assert_eq!(f.cursor, 3);
        f.move_left();
        assert_eq!(f.cursor, 2);
    }

    /// Shift+方向扩展选区；selected_text 与范围。
    #[test]
    fn shift_extends_selection() {
        let mut f = Field::default();
        f.set("abcd");
        f.collapse_to(2);
        assert_eq!(field_key(&mut f, &ks("left")), FieldResult::Changed); // 无 shift：收敛
        assert!(!f.has_selection());
        // 模拟 Shift+Left 两次
        let mut shift_ks = ks("left");
        shift_ks.modifiers.shift = true;
        f.cursor = 2;
        f.anchor = 2;
        assert_eq!(field_key(&mut f, &shift_ks), FieldResult::Changed);
        assert_eq!(field_key(&mut f, &shift_ks), FieldResult::Changed);
        assert_eq!(f.selected_text(), Some("ab"));
        // 全选
        let mut cmd_a = ks("a");
        cmd_a.modifiers.platform = true;
        assert_eq!(field_key(&mut f, &cmd_a), FieldResult::Changed);
        assert_eq!(f.selected_text(), Some("abcd"));
    }

    /// 全选/剪切/复制/粘贴信号。
    #[test]
    fn clipboard_signals() {
        let mut f = Field::default();
        f.set("abc");
        let mut cmd_c = ks("c");
        cmd_c.modifiers.platform = true;
        assert_eq!(
            field_key(&mut f, &cmd_c),
            FieldResult::Clipboard(ClipOp::Copy)
        );
        let mut cmd_x = ks("x");
        cmd_x.modifiers.control = true; // 非 mac 平台用 Ctrl
        assert_eq!(
            field_key(&mut f, &cmd_x),
            FieldResult::Clipboard(ClipOp::Cut)
        );
        let mut cmd_v = ks("v");
        cmd_v.modifiers.platform = true;
        assert_eq!(
            field_key(&mut f, &cmd_v),
            FieldResult::Clipboard(ClipOp::Paste)
        );
    }

    /// 粘贴：单行化、选区替换、容量截断。
    #[test]
    fn paste_behavior() {
        let mut f = Field::default();
        f.set("ab");
        f.collapse_to(1);
        assert!(f.paste("x\r\ny"));
        assert_eq!(f.as_str(), "axyb"); // 换行被去掉

        let mut f = Field::default();
        f.set("hello world");
        f.anchor = 5;
        f.cursor = 11; // 选 " world"
        assert!(f.paste("!"));
        assert_eq!(f.as_str(), "hello!");

        // 容量截断
        let mut f = Field::default();
        f.set(&"a".repeat(195));
        f.collapse_to(0);
        assert!(f.paste(&"b".repeat(50)));
        assert_eq!(f.char_len(), 200);
    }

    /// 双击选词：字母数字连续段；非词字符仅定位。
    #[test]
    fn word_selection() {
        let mut f = Field::default();
        f.set("hello world");
        f.select_word(7); // 落在 "world" 内
        assert_eq!(f.selected_text(), Some("world"));
        f.select_word(5); // 空格：仅定位
        assert!(!f.has_selection());
        assert_eq!(f.cursor, 5);
        // 汉字也是字母数字连续段：整段「密码abc」算一个词（与 macOS 原生行为一致）
        let mut f = Field::default();
        f.set("密码abc");
        f.select_word(1);
        assert_eq!(f.selected_text(), Some("密码abc"));
        // 中文与英文之间用空格/符号分隔时才断开
        let mut f = Field::default();
        f.set("密码 abc");
        f.select_word(1);
        assert_eq!(f.selected_text(), Some("密码"));
    }

    /// 渲染分段：无选区两段光标在中间；有选区三段、光标在活动端。
    #[test]
    fn render_segments_shape() {
        let mut f = Field::default();
        f.set("abcd");
        f.collapse_to(2);
        let (segs, caret_at) = f.render_segments();
        assert_eq!(segs, vec!["ab", "cd"]);
        assert_eq!(caret_at, 1);

        f.anchor = 2;
        f.cursor = 4; // 选 "cd"，光标在尾
        let (segs, caret_at) = f.render_segments();
        assert_eq!(segs, vec!["ab", "cd", ""]);
        assert_eq!(caret_at, 2);

        f.anchor = 4;
        f.cursor = 2; // 光标在头
        let (segs, caret_at) = f.render_segments();
        assert_eq!(segs, vec!["ab", "cd", ""]);
        assert_eq!(caret_at, 1);
    }

    /// 中文 char 语义 + key_char 大写优先。
    #[test]
    fn unicode_and_key_char() {
        let mut f = Field::default();
        f.set("密码ab");
        f.collapse_to(2);
        assert_eq!(f.split(), ("密码", "ab"));

        // Shift+A：key="a"、key_char="A" → 应插入大写
        let mut f = Field::default();
        assert_eq!(field_key(&mut f, &ks_char("a", "A")), FieldResult::Changed);
        assert_eq!(f.as_str(), "A");
    }
}
