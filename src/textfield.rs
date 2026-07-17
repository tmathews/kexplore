//! Single-line text-field editing state for the URL bar: caret + selection
//! over a String, with char/word movement. Byte indices always sit on char
//! boundaries.

pub struct TextField {
    pub active: bool,
    pub text: String,
    /// Caret byte index.
    pub caret: usize,
    /// Selection anchor byte index; selection is anchor..caret (unordered).
    pub anchor: Option<usize>,
    /// Horizontal scroll in logical px so the caret stays visible.
    pub scroll: f32,
}

impl TextField {
    pub fn new() -> TextField {
        TextField { active: false, text: String::new(), caret: 0, anchor: None, scroll: 0.0 }
    }

    pub fn begin(&mut self, text: String, caret: usize) {
        self.caret = caret.min(text.len());
        while !text.is_char_boundary(self.caret) {
            self.caret -= 1;
        }
        self.text = text;
        self.active = true;
        self.anchor = None;
        self.scroll = 0.0;
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.text.clear();
        self.anchor = None;
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.caret {
            return None;
        }
        Some((a.min(self.caret), a.max(self.caret)))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(s, e)| &self.text[s..e])
    }

    fn prev_boundary(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 {
            i -= 1;
            if self.text.is_char_boundary(i) {
                return i;
            }
        }
        0
    }

    fn next_boundary(&self, from: usize) -> usize {
        let mut i = from;
        while i < self.text.len() {
            i += 1;
            if self.text.is_char_boundary(i) {
                return i;
            }
        }
        self.text.len()
    }

    /// Word boundaries treat path separators and punctuation as breaks.
    fn prev_word(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && !is_word(self.char_before(i)) {
            i = self.prev_boundary(i);
        }
        while i > 0 && is_word(self.char_before(i)) {
            i = self.prev_boundary(i);
        }
        i
    }

    fn next_word(&self, from: usize) -> usize {
        let mut i = from;
        while i < self.text.len() && !is_word(self.char_at(i)) {
            i = self.next_boundary(i);
        }
        while i < self.text.len() && is_word(self.char_at(i)) {
            i = self.next_boundary(i);
        }
        i
    }

    fn char_at(&self, i: usize) -> char {
        self.text[i..].chars().next().unwrap_or('\0')
    }

    fn char_before(&self, i: usize) -> char {
        self.text[..i].chars().next_back().unwrap_or('\0')
    }

    fn update_anchor(&mut self, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
    }

    pub fn move_left(&mut self, word: bool, select: bool) {
        self.update_anchor(select);
        if !select && self.selection().is_some() {
            // Collapse to selection start like normal editors.
            self.caret = self.selection().unwrap().0;
            self.anchor = None;
            return;
        }
        self.caret = if word { self.prev_word(self.caret) } else { self.prev_boundary(self.caret) };
    }

    pub fn move_right(&mut self, word: bool, select: bool) {
        self.update_anchor(select);
        if !select && self.selection().is_some() {
            self.caret = self.selection().unwrap().1;
            self.anchor = None;
            return;
        }
        self.caret = if word { self.next_word(self.caret) } else { self.next_boundary(self.caret) };
    }

    pub fn move_home(&mut self, select: bool) {
        self.update_anchor(select);
        self.caret = 0;
    }

    pub fn move_end(&mut self, select: bool) {
        self.update_anchor(select);
        self.caret = self.text.len();
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
    }

    /// Remove the selection if any; returns true if something was removed.
    pub fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection() {
            self.text.replace_range(s..e, "");
            self.caret = s;
            self.anchor = None;
            true
        } else {
            false
        }
    }

    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_selection() {
            return;
        }
        let to = if word { self.prev_word(self.caret) } else { self.prev_boundary(self.caret) };
        self.text.replace_range(to..self.caret, "");
        self.caret = to;
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let to = self.next_boundary(self.caret);
        self.text.replace_range(self.caret..to, "");
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}
