//! Native, controlled text editing for Zolo desktop components.
//!
//! Input events must be queued by the bridge. Their callback runs under a GPUI
//! entity borrow, so it must never call back into the Zolo runtime directly.
use std::{cell::Cell, ops::Range, rc::Rc, time::Duration};

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, rgba, size, App, Bounds, ClipboardItem,
    ContentMask, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, InspectorElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent,
    SharedString, Style, Task, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
    WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    zolo_text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocumentStart,
        DocumentEnd,
        SelectDocumentStart,
        SelectDocumentEnd,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        Enter,
        Newline,
        NextFocus,
        PreviousFocus,
        ShowCharacterPalette,
    ]
);

/// Register once when the application starts. Bindings apply only to focused inputs.
pub fn init(cx: &mut App) {
    let context = Some("ZoloTextInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("ctrl-left", WordLeft, context),
        KeyBinding::new("ctrl-right", WordRight, context),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, context),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, context),
        KeyBinding::new("alt-left", WordLeft, context),
        KeyBinding::new("alt-right", WordRight, context),
        KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SelectWordRight, context),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, context),
        KeyBinding::new("ctrl-delete", DeleteWordRight, context),
        KeyBinding::new("alt-backspace", DeleteWordLeft, context),
        KeyBinding::new("alt-delete", DeleteWordRight, context),
        KeyBinding::new("ctrl-home", DocumentStart, context),
        KeyBinding::new("ctrl-end", DocumentEnd, context),
        KeyBinding::new("ctrl-shift-home", SelectDocumentStart, context),
        KeyBinding::new("ctrl-shift-end", SelectDocumentEnd, context),
        KeyBinding::new("cmd-up", DocumentStart, context),
        KeyBinding::new("cmd-down", DocumentEnd, context),
        KeyBinding::new("cmd-shift-up", SelectDocumentStart, context),
        KeyBinding::new("cmd-shift-down", SelectDocumentEnd, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("ctrl-z", Undo, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("ctrl-y", Redo, context),
        KeyBinding::new("ctrl-shift-z", Redo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
        KeyBinding::new("enter", Enter, context),
        KeyBinding::new("shift-enter", Newline, context),
        KeyBinding::new("tab", NextFocus, context),
        KeyBinding::new("shift-tab", PreviousFocus, context),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, context),
    ]);
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputProps {
    pub value: String,
    pub placeholder: String,
    pub multiline: bool,
    pub disabled: bool,
    pub read_only: bool,
    /// Enter submits when true. Shift+Enter still inserts a multiline newline.
    pub submit_on_enter: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputEvent {
    /// "change" or "submit".
    pub kind: String,
    pub text: String,
    /// Native acknowledgement token; not part of the public Zolo input event.
    pub(crate) edit_sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
    reversed: bool,
}

#[derive(Default)]
struct Editing {
    text: String,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    composition_start: Option<Snapshot>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

/// A cached native paint is not a host acknowledgement. Only a newly published
/// tree may accept, transform or reject committed edits.
struct HostRevision {
    revision: u64,
    value: String,
    latest_edit: u64,
    acknowledged_edit: u64,
}

impl HostRevision {
    fn new(value: String) -> Self {
        Self {
            revision: 0,
            value,
            latest_edit: 0,
            acknowledged_edit: 0,
        }
    }
    fn emitted(&mut self, sequence: u64) {
        self.latest_edit = sequence;
    }
    fn reconcile(
        &mut self,
        editing: &mut Editing,
        value: String,
        revision: u64,
        acknowledged_edit: u64,
        cancel_composition: bool,
    ) -> bool {
        if self.revision == revision
            && self.value == value
            && self.acknowledged_edit == acknowledged_edit
            && !cancel_composition
        {
            return false;
        }
        // IME preedit is local until commit. An unrelated host publication must
        // retain it; an explicit new host value still overrides composition.
        let retain_preedit = !cancel_composition
            && editing.marked.is_some()
            && (value == self.value
                || editing
                    .composition_start
                    .as_ref()
                    .is_some_and(|start| start.text == value));
        let override_preedit = editing.marked.is_some() && value != self.value;
        self.revision = revision;
        self.acknowledged_edit = acknowledged_edit;
        self.value = value.clone();
        // This publication may have been prepared before a newer native edit.
        // Its frozen batch acknowledgement cannot accept/reject that edit yet.
        if retain_preedit || (!cancel_composition && self.latest_edit > acknowledged_edit) {
            return false;
        }
        let changed = editing.text != value
            || ((cancel_composition || override_preedit) && editing.marked.is_some());
        if cancel_composition || override_preedit {
            editing.marked = None;
            editing.composition_start = None;
        }
        editing.external_value(value);
        changed
    }
}

fn normalize_text(text: &str, multiline: bool) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if multiline {
        text
    } else {
        text.replace(['\n', '\t'], " ")
    }
}

fn utf16_to_byte(text: &str, offset: usize, round_up: bool) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if offset <= units {
            return byte;
        }
        let next = units + ch.len_utf16();
        if offset < next {
            return if round_up { byte + ch.len_utf8() } else { byte };
        }
        units = next;
    }
    text.len()
}

fn byte_to_utf16(text: &str, offset: usize) -> usize {
    text.char_indices()
        .take_while(|(byte, _)| *byte < offset)
        .map(|(_, ch)| ch.len_utf16())
        .sum()
}

fn from_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    let start = range.start.min(range.end);
    let end = range.start.max(range.end);
    let start_byte = utf16_to_byte(text, start, false);
    // Empty selections in a surrogate pair round down without deleting the pair.
    let end_byte = if start == end {
        start_byte
    } else {
        utf16_to_byte(text, end, true)
    };
    start_byte..end_byte
}

fn to_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    byte_to_utf16(text, range.start)..byte_to_utf16(text, range.end)
}

fn previous_grapheme(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(byte, _)| (byte < offset).then_some(byte))
        .unwrap_or(0)
}

fn next_grapheme(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(byte, _)| (byte > offset).then_some(byte))
        .unwrap_or(text.len())
}

fn clamp_grapheme(text: &str, offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .take_while(|(byte, _)| *byte <= offset)
        .last()
        .map_or(0, |(byte, _)| byte)
}

impl Editing {
    fn new(text: String) -> Self {
        Self {
            selection: text.len()..text.len(),
            text,
            ..Self::default()
        }
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            reversed: self.reversed,
        }
    }
    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }
    fn move_to(&mut self, offset: usize, selecting: bool) {
        let offset = clamp_grapheme(&self.text, offset);
        if selecting {
            let anchor = if self.reversed {
                self.selection.end
            } else {
                self.selection.start
            };
            self.selection = anchor.min(offset)..anchor.max(offset);
            self.reversed = offset < anchor;
        } else {
            self.selection = offset..offset;
            self.reversed = false;
        }
    }
    fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo.last() != Some(&snapshot) {
            self.undo.push(snapshot);
        }
        // Bound undo memory independently of the application's lifetime.
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn commit_composition(&mut self) -> bool {
        self.marked = None;
        if let Some(snapshot) = self.composition_start.take() {
            if snapshot.text != self.text {
                self.push_undo(snapshot);
                return true;
            }
        }
        false
    }
    fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        composing: bool,
        selection_utf16: Option<Range<usize>>,
    ) -> bool {
        let old = self.snapshot();
        if composing {
            if self.composition_start.is_none() {
                self.composition_start = Some(old.clone());
            }
        } else if self.composition_start.is_none() {
            if self.text[range.clone()] != *text {
                self.push_undo(old.clone());
            }
        }
        self.text.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selection = if composing {
            selection_utf16
                .map(|relative| {
                    let selected = from_utf16(text, relative);
                    range.start + selected.start..range.start + selected.end
                })
                .unwrap_or(end..end)
        } else {
            end..end
        };
        self.reversed = false;
        if composing && !text.is_empty() {
            self.marked = Some(range.start..end);
        } else {
            self.commit_composition();
        }
        old.text != self.text
    }
    fn replace_committed(&mut self, range: Range<usize>, text: &str) -> bool {
        // Committing the exact last preedit still changes the host value, even
        // though the visible text does not change in this individual call.
        let before = self
            .composition_start
            .as_ref()
            .map_or_else(|| self.text.clone(), |snapshot| snapshot.text.clone());
        self.replace(range, text, false, None);
        before != self.text
    }
    fn external_value(&mut self, text: String) {
        if text == self.text {
            return;
        }
        self.text = text;
        let start = clamp_grapheme(&self.text, self.selection.start);
        let end = clamp_grapheme(&self.text, self.selection.end);
        self.selection = start.min(end)..start.max(end);
        self.marked = None;
        self.composition_start = None;
        self.undo.clear();
        self.redo.clear();
    }
    fn history(&mut self, undo: bool) -> bool {
        self.commit_composition();
        let snapshot = if undo {
            self.undo.pop()
        } else {
            self.redo.pop()
        };
        let Some(snapshot) = snapshot else {
            return false;
        };
        let current = self.snapshot();
        if undo {
            self.redo.push(current);
        } else {
            self.undo.push(current);
        }
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.reversed = snapshot.reversed;
        true
    }
    fn word_boundary(&self, right: bool) -> usize {
        let cursor = self.cursor();
        if right {
            self.text
                .unicode_word_indices()
                .find_map(|(start, word)| {
                    (start + word.len() > cursor).then_some(start + word.len())
                })
                .unwrap_or(self.text.len())
        } else {
            self.text
                .unicode_word_indices()
                .rev()
                .find_map(|(start, _)| (start < cursor).then_some(start))
                .unwrap_or(0)
        }
    }
    fn line_boundary(&self, end: bool) -> usize {
        let cursor = self.cursor();
        if end {
            self.text[cursor..]
                .find('\n')
                .map_or(self.text.len(), |byte| cursor + byte)
        } else {
            self.text[..cursor].rfind('\n').map_or(0, |byte| byte + 1)
        }
    }
}

#[derive(Clone)]
struct LayoutLine {
    start: usize,
    line: WrappedLine,
    y: Pixels,
}

#[derive(Clone)]
struct TextLayout {
    text: String,
    lines: Vec<LayoutLine>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    scroll: Point<Pixels>,
    extent: gpui::Size<Pixels>,
}

impl TextLayout {
    fn position(&self, offset: usize) -> Point<Pixels> {
        let line = self
            .lines
            .iter()
            .rev()
            .find(|line| line.start <= offset)
            .or(self.lines.first());
        let Some(line) = line else {
            return point(px(0.), px(0.));
        };
        let local = (offset - line.start).min(line.line.len());
        let position = line
            .line
            .position_for_index(local, self.line_height)
            .unwrap_or_default();
        point(position.x, line.y + position.y)
    }
    fn index(&self, position: Point<Pixels>) -> usize {
        let Some(line) = self
            .lines
            .iter()
            .rev()
            .find(|line| line.y <= position.y)
            .or(self.lines.first())
        else {
            return 0;
        };
        let local = point(
            position.x,
            (position.y - line.y)
                .max(px(0.))
                .min(line.line.size(self.line_height).height - px(0.01)),
        );
        let index = line
            .line
            .closest_index_for_position(local, self.line_height)
            .unwrap_or_else(|index| index);
        clamp_grapheme(&self.text, (line.start + index).min(self.text.len()))
    }
    fn index_at_window_point(&self, position: Point<Pixels>) -> usize {
        self.index(position - self.bounds.origin + self.scroll)
    }
}

pub struct TextInput {
    focus_handle: FocusHandle,
    props: InputProps,
    editing: Editing,
    host: HostRevision,
    edit_sequence: Rc<Cell<u64>>,
    emit: Rc<dyn Fn(InputEvent)>,
    layout: Option<TextLayout>,
    selecting: bool,
    scroll: Point<Pixels>,
    reveal_cursor: bool,
    preferred_x: Option<Pixels>,
    viewport_height: f32,
    cursor_visible: bool,
    blink_epoch: u64,
    blink_task: Option<Task<()>>,
}

impl TextInput {
    pub fn new(
        props: InputProps,
        window: &mut Window,
        cx: &mut Context<Self>,
        emit: Rc<dyn Fn(InputEvent)>,
    ) -> Self {
        Self::with_sequence(props, window, cx, emit, Rc::new(Cell::new(0)))
    }
    /// The renderer shares a monotonically increasing counter across an entire
    /// window, so a remounted input cannot reuse an old acknowledgement token.
    pub(crate) fn with_sequence(
        props: InputProps,
        window: &mut Window,
        cx: &mut Context<Self>,
        emit: Rc<dyn Fn(InputEvent)>,
        edit_sequence: Rc<Cell<u64>>,
    ) -> Self {
        let editing = Editing::new(normalize_text(&props.value, props.multiline));
        let focus_handle = cx.focus_handle().tab_index(0).tab_stop(!props.disabled);
        cx.on_focus(&focus_handle, window, |input, window, cx| {
            input.start_blink(window, cx)
        })
        .detach();
        cx.on_blur(&focus_handle, window, |input, _, cx| {
            input.blink_task = None;
            input.cursor_visible = false;
            input.selecting = false;
            input.finish_composition(cx);
            cx.notify();
        })
        .detach();
        Self {
            focus_handle,
            host: HostRevision::new(editing.text.clone()),
            edit_sequence,
            props,
            editing,
            emit,
            layout: None,
            selecting: false,
            scroll: Point::default(),
            reveal_cursor: true,
            preferred_x: None,
            viewport_height: 0.,
            cursor_visible: true,
            blink_epoch: 0,
            blink_task: None,
        }
    }
    /// Explicit standalone host publication. Cached renderers should use
    /// `sync_revision` with a revision that changes only for a new host tree.
    pub fn sync(&mut self, props: InputProps, cx: &mut Context<Self>) {
        self.sync_revision(
            props,
            self.host.revision.wrapping_add(1),
            self.host.latest_edit,
            cx,
        );
    }
    /// A new host publication is authoritative even when props are unchanged:
    /// this is how controlled validation can reject an edit. Equal text echoes
    /// keep the caret/history, and unrelated publications preserve IME preedit.
    pub fn sync_revision(
        &mut self,
        props: InputProps,
        revision: u64,
        acknowledged_edit: u64,
        cx: &mut Context<Self>,
    ) {
        let props_changed = props != self.props;
        let cancel_composition = props.multiline != self.props.multiline
            || ((props.disabled || props.read_only)
                && !(self.props.disabled || self.props.read_only));
        let value_changed = self.host.reconcile(
            &mut self.editing,
            normalize_text(&props.value, props.multiline),
            revision,
            acknowledged_edit,
            cancel_composition,
        );
        if value_changed {
            self.reveal_cursor = true;
        }
        if !props_changed && !value_changed {
            return;
        }
        self.focus_handle = self.focus_handle.clone().tab_stop(!props.disabled);
        if props.disabled {
            self.blink_task = None;
            self.selecting = false;
        }
        self.props = props;
        cx.notify();
    }
    pub fn value(&self) -> &str {
        &self.editing.text
    }
    /// A positive height overrides the default 26px/120px input viewport.
    pub fn set_viewport_height(&mut self, height: f32, cx: &mut Context<Self>) {
        let height = if height.is_finite() {
            height.max(0.)
        } else {
            0.
        };
        if self.viewport_height != height {
            self.viewport_height = height;
            self.reveal_cursor = true;
            cx.notify();
        }
    }
    fn start_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.props.disabled {
            return;
        }
        self.cursor_visible = true;
        let mut epoch = self.blink_epoch;
        self.blink_task = Some(cx.spawn_in(window, async move |input, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(530))
                    .await;
                let active = cx
                    .update(|window, cx| {
                        input.update(cx, |input, cx| {
                            if input.props.disabled || !input.focus_handle.is_focused(window) {
                                return false;
                            }
                            if epoch != input.blink_epoch {
                                epoch = input.blink_epoch;
                                input.cursor_visible = true;
                            } else {
                                input.cursor_visible = !input.cursor_visible;
                            }
                            cx.notify();
                            true
                        })
                    })
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        }));
        cx.notify();
    }
    fn reset_blink(&mut self) {
        self.cursor_visible = true;
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
    }
    fn editable(&self) -> bool {
        !self.props.disabled && !self.props.read_only
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.reveal_cursor = true;
        self.preferred_x = None;
        self.reset_blink();
        let sequence = self
            .edit_sequence
            .get()
            .checked_add(1)
            .expect("native input edit sequence exhausted");
        // The bridge carries this internal token in an otherwise unused f64
        // field on change events; keep it exactly representable.
        assert!(
            sequence <= 9_007_199_254_740_991,
            "native input edit sequence exhausted"
        );
        self.edit_sequence.set(sequence);
        self.host.emitted(sequence);
        (self.emit)(InputEvent {
            kind: "change".into(),
            text: self.editing.text.clone(),
            edit_sequence: sequence,
        });
        cx.notify();
    }
    fn finish_composition(&mut self, cx: &mut Context<Self>) {
        if self.editing.commit_composition() {
            self.changed(cx);
        }
    }
    fn navigate(&mut self, offset: usize, selecting: bool, cx: &mut Context<Self>) {
        if self.props.disabled {
            return;
        }
        self.finish_composition(cx);
        self.editing.move_to(offset, selecting);
        self.preferred_x = None;
        self.reveal_cursor = true;
        self.reset_blink();
        cx.notify();
    }
    fn horizontal(&mut self, right: bool, selecting: bool, cx: &mut Context<Self>) {
        let offset = if !selecting && !self.editing.selection.is_empty() {
            if right {
                self.editing.selection.end
            } else {
                self.editing.selection.start
            }
        } else if right {
            next_grapheme(&self.editing.text, self.editing.cursor())
        } else {
            previous_grapheme(&self.editing.text, self.editing.cursor())
        };
        self.navigate(offset, selecting, cx);
    }
    fn vertical(&mut self, down: bool, selecting: bool, cx: &mut Context<Self>) {
        if self.props.disabled {
            return;
        }
        let Some(layout) = self
            .layout
            .as_ref()
            .filter(|layout| layout.text == self.editing.text)
        else {
            return;
        };
        let position = layout.position(self.editing.cursor());
        let preferred_x = self.preferred_x.unwrap_or(position.x);
        let target_y = (position.y
            + if down {
                layout.line_height
            } else {
                -layout.line_height
            })
        .max(px(0.));
        let offset = layout.index(point(preferred_x, target_y));
        self.navigate(offset, selecting, cx);
        self.preferred_x = Some(preferred_x);
    }
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(false, false, cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(true, false, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(false, true, cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(true, true, cx);
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, false, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, false, cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, true, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, true, cx);
    }
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.word_boundary(false), false, cx);
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.word_boundary(true), false, cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.word_boundary(false), true, cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.word_boundary(true), true, cx);
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.line_boundary(false), false, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.line_boundary(true), false, cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.line_boundary(false), true, cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.line_boundary(true), true, cx);
    }
    fn document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(0, false, cx);
    }
    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(self.editing.text.len(), false, cx);
    }
    fn select_document_start(
        &mut self,
        _: &SelectDocumentStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(0, true, cx);
    }
    fn select_document_end(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(self.editing.text.len(), true, cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(0, false, cx);
        self.navigate(self.editing.text.len(), true, cx);
    }
    fn delete_range(&mut self, right: bool, word: bool, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        self.finish_composition(cx);
        let range = if self.editing.selection.is_empty() {
            let cursor = self.editing.cursor();
            let offset = if word {
                self.editing.word_boundary(right)
            } else if right {
                next_grapheme(&self.editing.text, cursor)
            } else {
                previous_grapheme(&self.editing.text, cursor)
            };
            cursor.min(offset)..cursor.max(offset)
        } else {
            self.editing.selection.clone()
        };
        if self.editing.replace(range, "", false, None) {
            self.changed(cx);
        }
    }
    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_range(false, false, cx);
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_range(true, false, cx);
    }
    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_range(false, true, cx);
    }
    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_range(true, true, cx);
    }
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.props.disabled && !self.editing.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.editing.text[self.editing.selection.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        self.copy(&Copy, window, cx);
        if !self.editing.selection.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
        }
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.editable() && self.editing.history(true) {
            self.changed(cx);
        }
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.editable() && self.editing.history(false) {
            self.changed(cx);
        }
    }
    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() || self.editing.marked.is_some() {
            return;
        }
        if self.props.submit_on_enter || !self.props.multiline {
            (self.emit)(InputEvent {
                kind: "submit".into(),
                text: self.editing.text.clone(),
                edit_sequence: 0,
            });
        } else {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }
    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.props.multiline && self.editing.marked.is_none() {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }
    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if self.editable() {
            window.show_character_palette();
        }
    }
    fn next_focus(&mut self, _: &NextFocus, window: &mut Window, _: &mut Context<Self>) {
        window.focus_next();
    }
    fn previous_focus(&mut self, _: &PreviousFocus, window: &mut Window, _: &mut Context<Self>) {
        window.focus_prev();
    }
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.props.disabled {
            return;
        }
        window.focus(&self.focus_handle);
        let index = self
            .layout
            .as_ref()
            .filter(|layout| layout.text == self.editing.text)
            .map_or(0, |layout| layout.index_at_window_point(event.position));
        self.selecting = true;
        if event.click_count >= 3 {
            self.navigate(index, false, cx);
            let start = self.editing.line_boundary(false);
            let end = self.editing.line_boundary(true);
            let end = if end < self.editing.text.len() {
                end + 1
            } else {
                end
            };
            self.navigate(start, false, cx);
            self.navigate(end, true, cx);
        } else if event.click_count == 2 {
            let word = self
                .editing
                .text
                .unicode_word_indices()
                .find(|(start, word)| *start <= index && index < *start + word.len())
                .map(|(start, word)| start..start + word.len());
            if let Some(word) = word {
                self.navigate(word.start, false, cx);
                self.navigate(word.end, true, cx);
            } else {
                self.navigate(index, false, cx);
            }
        } else {
            self.navigate(index, event.modifiers.shift, cx);
        }
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.dragging() {
            if let Some(layout) = self
                .layout
                .as_ref()
                .filter(|layout| layout.text == self.editing.text)
            {
                self.navigate(layout.index_at_window_point(event.position), true, cx);
            }
        }
    }
    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let delta = event.delta.pixel_delta(window.line_height());
        let max = point(
            (layout.extent.width - layout.bounds.size.width).max(px(0.)),
            (layout.extent.height - layout.bounds.size.height).max(px(0.)),
        );
        let scroll = point(
            (self.scroll.x - delta.x).max(px(0.)).min(max.x),
            (self.scroll.y - delta.y).max(px(0.)).min(max.y),
        );
        if self.scroll != scroll {
            self.scroll = scroll;
            self.reveal_cursor = false;
            cx.stop_propagation();
            cx.notify();
        }
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if self.props.disabled {
            return None;
        }
        let range = from_utf16(&self.editing.text, range);
        *actual_range = Some(to_utf16(&self.editing.text, range.clone()));
        Some(self.editing.text[range].to_string())
    }
    fn selected_text_range(
        &mut self,
        ignore_disabled: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !ignore_disabled && !self.editable() {
            return None;
        }
        Some(UTF16Selection {
            range: to_utf16(&self.editing.text, self.editing.selection.clone()),
            reversed: self.editing.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.editing
            .marked
            .clone()
            .map(|range| to_utf16(&self.editing.text, range))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() {
            return;
        }
        let range = range
            .map(|range| from_utf16(&self.editing.text, range))
            .or(self.editing.marked.clone())
            .unwrap_or(self.editing.selection.clone());
        let text = normalize_text(text, self.props.multiline);
        if self.editing.replace_committed(range, &text) {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() {
            return;
        }
        let range = range
            .map(|range| from_utf16(&self.editing.text, range))
            .or(self.editing.marked.clone())
            .unwrap_or(self.editing.selection.clone());
        let text = normalize_text(text, self.props.multiline);
        let before = self.editing.composition_start.as_ref().map_or_else(
            || self.editing.text.clone(),
            |snapshot| snapshot.text.clone(),
        );
        self.editing.replace(range, &text, true, selected);
        if self.editing.marked.is_none() && before != self.editing.text {
            self.changed(cx);
            return;
        }
        self.reveal_cursor = true;
        self.preferred_x = None;
        self.reset_blink();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self
            .layout
            .as_ref()
            .filter(|layout| layout.text == self.editing.text)?;
        let range = from_utf16(&self.editing.text, range);
        let start = layout.position(range.start);
        let end = layout.position(range.end);
        let width = if start.y == end.y {
            (end.x - start.x).max(px(1.))
        } else {
            (layout.bounds.size.width - start.x).max(px(1.))
        };
        Some(Bounds::new(
            layout.bounds.origin + start - layout.scroll,
            size(width, layout.line_height),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self
            .layout
            .as_ref()
            .filter(|layout| layout.text == self.editing.text)?;
        if !layout.bounds.contains(&point) {
            return None;
        }
        Some(byte_to_utf16(
            &self.editing.text,
            layout.index_at_window_point(point),
        ))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}
struct PrepaintState {
    layout: TextLayout,
    selections: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let input = self.input.read(cx);
        let placeholder = input.editing.text.is_empty();
        let display: SharedString = if placeholder {
            input.props.placeholder.clone().into()
        } else {
            input.editing.text.clone().into()
        };
        let style = window.text_style();
        let mut color = style.color;
        if placeholder || input.props.disabled {
            color.a *= 0.45;
        }
        let run = TextRun {
            len: display.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let mut runs = vec![];
        if let Some(marked) = input.editing.marked.as_ref() {
            for (range, underline) in [
                (0..marked.start, false),
                (marked.clone(), true),
                (marked.end..display.len(), false),
            ] {
                if !range.is_empty() {
                    runs.push(TextRun {
                        len: range.len(),
                        underline: underline.then_some(UnderlineStyle {
                            color: Some(color),
                            thickness: px(1.),
                            wavy: false,
                        }),
                        ..run.clone()
                    });
                }
            }
        } else if !display.is_empty() {
            runs.push(run);
        }
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let wrapped = window
            .text_system()
            .shape_text(
                display.clone(),
                font_size,
                &runs,
                input
                    .props
                    .multiline
                    .then_some(bounds.size.width.max(px(1.))),
                None,
            )
            .unwrap_or_default();
        let mut lines = vec![];
        let mut start = 0;
        let mut y = px(0.);
        let mut width = bounds.size.width;
        for line in wrapped {
            width = width.max(line.width());
            let height = line.size(line_height).height.max(line_height);
            let len = line.len();
            lines.push(LayoutLine { start, line, y });
            start += len + 1;
            y += height;
        }
        let extent = size(width, y.max(line_height));
        let mut layout = TextLayout {
            text: input.editing.text.clone(),
            lines,
            bounds,
            line_height,
            scroll: input.scroll,
            extent,
        };
        let cursor_position = if placeholder {
            point(px(0.), px(0.))
        } else {
            layout.position(input.editing.cursor())
        };
        if input.reveal_cursor {
            if cursor_position.x < layout.scroll.x {
                layout.scroll.x = cursor_position.x;
            }
            if cursor_position.x + px(2.) > layout.scroll.x + bounds.size.width {
                layout.scroll.x = cursor_position.x + px(2.) - bounds.size.width;
            }
            if cursor_position.y < layout.scroll.y {
                layout.scroll.y = cursor_position.y;
            }
            if cursor_position.y + line_height > layout.scroll.y + bounds.size.height {
                layout.scroll.y = cursor_position.y + line_height - bounds.size.height;
            }
        }
        layout.scroll.x = layout
            .scroll
            .x
            .max(px(0.))
            .min((extent.width + px(2.) - bounds.size.width).max(px(0.)));
        layout.scroll.y = layout
            .scroll
            .y
            .max(px(0.))
            .min((extent.height - bounds.size.height).max(px(0.)));
        let mut selections = vec![];
        if !placeholder && !input.editing.selection.is_empty() {
            let selection = input.editing.selection.clone();
            for line in &layout.lines {
                // Iterate visual rows; wrapped glyph positions provide row boundaries.
                let row_count = line.line.wrap_boundaries().len() + 1;
                for row in 0..row_count {
                    let row_y = line_height * row as f32;
                    let row_start = line
                        .line
                        .closest_index_for_position(point(px(-1.), row_y), line_height)
                        .unwrap_or_else(|index| index);
                    let row_end = line
                        .line
                        .closest_index_for_position(point(px(f32::MAX), row_y), line_height)
                        .unwrap_or_else(|index| index);
                    let start = selection.start.max(line.start + row_start);
                    let end = selection.end.min(line.start + row_end);
                    let includes_newline = row + 1 == row_count
                        && selection.start <= line.start + row_end
                        && selection.end > line.start + row_end;
                    if start < end || includes_newline {
                        let a = line
                            .line
                            .position_for_index(
                                start.saturating_sub(line.start).min(line.line.len()),
                                line_height,
                            )
                            .unwrap_or_default();
                        let b = line
                            .line
                            .position_for_index(
                                end.saturating_sub(line.start).min(line.line.len()),
                                line_height,
                            )
                            .unwrap_or_default();
                        let left = if start == line.start + row_start {
                            px(0.)
                        } else {
                            a.x
                        };
                        let right = if includes_newline || b.y != row_y {
                            bounds.size.width.max(left + px(2.))
                        } else {
                            b.x.max(left + px(2.))
                        };
                        selections.push(fill(
                            Bounds::new(
                                bounds.origin + point(left, line.y + row_y) - layout.scroll,
                                size(right - left, line_height),
                            ),
                            rgba(0x5b8def55),
                        ));
                    }
                }
            }
        }
        let cursor = (input.editing.selection.is_empty()
            && input.focus_handle.is_focused(window)
            && !input.props.disabled
            && input.cursor_visible)
            .then(|| {
                fill(
                    Bounds::new(
                        bounds.origin + cursor_position - layout.scroll,
                        size(px(1.5), line_height),
                    ),
                    color,
                )
            });
        PrepaintState {
            layout,
            selections,
            cursor,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let input = self.input.read(cx);
        let focus_handle = input.focus_handle.clone();
        if !input.props.disabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for selection in state.selections.drain(..) {
                window.paint_quad(selection);
            }
            for line in &state.layout.lines {
                let origin = bounds.origin + point(px(0.), line.y) - state.layout.scroll;
                // Layout shape errors are recoverable; do not unwind under a native entity borrow.
                let _ = line.line.paint(
                    origin,
                    state.layout.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some(cursor) = state.cursor.take() {
                window.paint_quad(cursor);
            }
        });
        self.input.update(cx, |input, _| {
            input.scroll = state.layout.scroll;
            input.layout = Some(state.layout.clone());
            input.reveal_cursor = false;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = if self.viewport_height > 0. {
            self.viewport_height
        } else if self.props.multiline {
            120.
        } else {
            26.
        };
        div()
            .w_full()
            .h(px(height))
            .min_w(px(0.))
            .overflow_hidden()
            .key_context("ZoloTextInput")
            .track_focus(&self.focus_handle)
            .cursor(if self.props.disabled {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::document_start))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_document_start))
            .on_action(cx.listener(Self::select_document_end))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::next_focus))
            .on_action(cx.listener(Self::previous_focus))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_cover_surrogates_without_slicing_utf8() {
        let text = "a🦀éz";
        assert_eq!(from_utf16(text, 1..3), 1..5);
        assert_eq!(from_utf16(text, 2..3), 1..5);
        assert_eq!(from_utf16(text, 2..2), 1..1);
        assert_eq!(to_utf16(text, 1..7), 1..4);
        assert_eq!(from_utf16(text, 90..100), text.len()..text.len());
    }

    #[test]
    fn navigation_and_deletion_keep_extended_graphemes_intact() {
        for text in ["e\u{301}", "👩‍💻", "🇧🇷"] {
            let input = format!("x{text}y");
            assert_eq!(next_grapheme(&input, 1), 1 + text.len());
            assert_eq!(previous_grapheme(&input, 1 + text.len()), 1);
            let mut edit = Editing::new(input);
            edit.move_to(1 + text.len(), false);
            edit.move_to(previous_grapheme(&edit.text, edit.cursor()), true);
            edit.replace(edit.selection.clone(), "", false, None);
            assert_eq!(edit.text, "xy");
        }
    }

    #[test]
    fn composition_selection_is_relative_to_inserted_text() {
        let mut edit = Editing::new("before🦀after".into());
        assert!(edit.replace(6..10, "日本", true, Some(1..2)));
        assert_eq!(edit.text, "before日本after");
        assert_eq!(edit.marked, Some(6..12));
        assert_eq!(edit.selection, 9..12);
        edit.replace(6..12, "日本語", true, Some(2..3));
        assert_eq!(edit.selection, 12..15);
        edit.commit_composition();
        assert!(edit.history(true));
        assert_eq!(edit.text, "before🦀after");
        assert!(edit.history(false));
        assert_eq!(edit.text, "before日本語after");
    }

    #[test]
    fn controlled_echo_preserves_composition_and_history() {
        let mut edit = Editing::new("a".into());
        edit.replace(1..1, "é", true, Some(1..1));
        let selection = edit.selection.clone();
        edit.external_value("aé".into());
        assert_eq!(edit.selection, selection);
        assert_eq!(edit.marked, Some(1..3));
        edit.commit_composition();
        assert!(edit.history(true));
        assert_eq!(edit.text, "a");
    }

    #[test]
    fn cached_paints_retain_edits_but_a_new_host_revision_can_reject_them() {
        let mut edit = Editing::new("12".into());
        let mut host = HostRevision::new("12".into());
        host.reconcile(&mut edit, "12".into(), 1, 0, false);
        edit.replace_committed(2..2, "x");
        assert!(!host.reconcile(&mut edit, "12".into(), 1, 0, false));
        assert_eq!(edit.text, "12x");
        assert!(host.reconcile(&mut edit, "12".into(), 2, 0, false));
        assert_eq!(edit.text, "12");
        assert_eq!(edit.selection, 2..2);
        assert!(!edit.history(true));
    }

    #[test]
    fn publication_for_first_edit_cannot_erase_a_later_queued_edit() {
        let mut edit = Editing::new(String::new());
        let mut host = HostRevision::new(String::new());
        host.reconcile(&mut edit, String::new(), 1, 0, false);
        edit.replace_committed(0..0, "a");
        host.emitted(1);
        // The driver has dispatched 'a' and frozen this acknowledgement. Before
        // its tree is painted, the native input emits 'ab' into the next batch.
        let publication_ack = 1;
        edit.replace_committed(1..1, "b");
        host.emitted(2);
        assert!(!host.reconcile(&mut edit, "a".into(), 2, publication_ack, false));
        assert_eq!(edit.text, "ab");
        assert_eq!(edit.selection, 2..2);
        assert_eq!(edit.undo.len(), 2);
        assert!(!host.reconcile(&mut edit, "a".into(), 2, publication_ack, false));
        assert!(!host.reconcile(&mut edit, "ab".into(), 3, 2, false));
        assert_eq!(edit.selection, 2..2);
        assert!(edit.history(true));
        assert_eq!(edit.text, "a");
    }

    #[test]
    fn rejection_waits_for_the_edit_it_acknowledges() {
        let mut edit = Editing::new("12".into());
        let mut host = HostRevision::new("12".into());
        host.reconcile(&mut edit, "12".into(), 1, 0, false);
        edit.replace_committed(2..2, "x");
        host.emitted(1);
        edit.replace_committed(3..3, "y");
        host.emitted(2);
        // Rejecting edit 1 must not discard edit 2 before it reaches validation.
        assert!(!host.reconcile(&mut edit, "12".into(), 2, 1, false));
        assert_eq!(edit.text, "12xy");
        // Equal props are still authoritative once edit 2 has been processed.
        assert!(host.reconcile(&mut edit, "12".into(), 3, 2, false));
        assert_eq!(edit.text, "12");
        assert_eq!(edit.selection, 2..2);
        assert!(!edit.history(true));
    }

    #[test]
    fn acknowledgement_of_preceding_edit_preserves_new_ime_composition() {
        let mut edit = Editing::new(String::new());
        let mut host = HostRevision::new(String::new());
        host.reconcile(&mut edit, String::new(), 1, 0, false);
        edit.replace_committed(0..0, "a");
        host.emitted(1);
        edit.replace(1..1, "\u{65e5}", true, Some(1..1));
        assert!(!host.reconcile(&mut edit, "a".into(), 2, 1, false));
        assert_eq!(edit.text, "a\u{65e5}");
        assert!(edit.marked.is_some());
        assert_eq!(edit.undo.len(), 1);
        assert!(edit.commit_composition());
        host.emitted(2);
        assert!(!host.reconcile(&mut edit, "a".into(), 3, 1, false));
        assert!(host.reconcile(&mut edit, "a".into(), 4, 2, false));
        assert_eq!(edit.text, "a");
    }

    #[test]
    fn remounted_input_uses_fresh_sequence_beyond_old_owner_acknowledgements() {
        let mut edit = Editing::new("host".into());
        let mut host = HostRevision::new("host".into());
        host.reconcile(&mut edit, "host".into(), 1, 8, false);
        edit.replace_committed(4..4, "!");
        // The window counter survives the old entity's removal.
        host.emitted(9);
        assert!(!host.reconcile(&mut edit, "host".into(), 2, 8, false));
        assert_eq!(edit.text, "host!");
        assert!(host.reconcile(&mut edit, "host".into(), 3, 9, false));
        assert_eq!(edit.text, "host");
    }

    #[test]
    fn accepted_host_echo_keeps_selection_and_undo_transaction() {
        let mut edit = Editing::new("12".into());
        let mut host = HostRevision::new("12".into());
        host.reconcile(&mut edit, "12".into(), 1, 0, false);
        edit.replace_committed(2..2, "x");
        edit.move_to(1, false);
        assert!(!host.reconcile(&mut edit, "12x".into(), 2, 0, false));
        assert_eq!(edit.selection, 1..1);
        assert!(edit.history(true));
        assert_eq!(edit.text, "12");
    }

    #[test]
    fn unrelated_host_publication_preserves_uncommitted_ime() {
        let mut edit = Editing::new("12".into());
        let mut host = HostRevision::new("12".into());
        host.reconcile(&mut edit, "12".into(), 1, 0, false);
        edit.replace(2..2, "\u{65e5}\u{672c}", true, Some(1..2));
        let selection = edit.selection.clone();
        assert!(!host.reconcile(&mut edit, "12".into(), 2, 0, false));
        assert_eq!(edit.text, "12\u{65e5}\u{672c}");
        assert_eq!(edit.selection, selection);
        assert!(edit.marked.is_some());
        assert!(host.reconcile(&mut edit, "explicit".into(), 3, 0, false));
        assert_eq!(edit.text, "explicit");
        assert!(edit.marked.is_none());
        assert!(edit.composition_start.is_none());
    }

    #[test]
    fn final_ime_commit_reports_change_even_when_preedit_text_is_identical() {
        let mut edit = Editing::new("12".into());
        let mut host = HostRevision::new("12".into());
        host.reconcile(&mut edit, "12".into(), 1, 0, false);
        edit.replace(2..2, "\u{65e5}", true, Some(1..1));
        let range = edit.marked.clone().unwrap();
        assert!(edit.replace_committed(range, "\u{65e5}"));
        assert!(edit.marked.is_none());
        assert_eq!(edit.undo.len(), 1);
        assert!(host.reconcile(&mut edit, "12".into(), 2, 0, false));
        assert_eq!(edit.text, "12");
    }

    #[test]
    fn explicit_host_replacement_can_cancel_identical_visible_preedit() {
        let mut edit = Editing::new("a".into());
        let mut host = HostRevision::new("a".into());
        host.reconcile(&mut edit, "a".into(), 1, 0, false);
        edit.replace(1..1, "b", true, None);
        assert!(host.reconcile(&mut edit, "ab".into(), 2, 0, false));
        assert_eq!(edit.text, "ab");
        assert!(edit.marked.is_none());
        assert!(edit.composition_start.is_none());
        assert!(!edit.commit_composition());
    }

    #[test]
    fn unmark_commits_once_and_read_only_transition_cancels_preedit() {
        let mut edit = Editing::new("a".into());
        edit.replace(1..1, "b", true, None);
        assert!(edit.commit_composition());
        assert!(!edit.commit_composition());
        let mut host = HostRevision::new("ab".into());
        host.reconcile(&mut edit, "ab".into(), 1, 0, false);
        edit.replace(2..2, "c", true, None);
        assert!(host.reconcile(&mut edit, "ab".into(), 1, 0, true));
        assert_eq!(edit.text, "ab");
        assert!(edit.marked.is_none());
        assert!(edit.composition_start.is_none());
    }

    #[test]
    fn external_replacement_clamps_selection_and_clears_stale_history() {
        let mut edit = Editing::new("long text".into());
        edit.replace(9..9, "!", false, None);
        edit.external_value("🦀".into());
        assert_eq!(edit.selection, 4..4);
        assert!(!edit.history(true));
    }

    #[test]
    fn selection_reversal_keeps_original_anchor() {
        let mut edit = Editing::new("abcde".into());
        edit.move_to(3, false);
        edit.move_to(1, true);
        assert_eq!(edit.selection, 1..3);
        assert!(edit.reversed);
        edit.move_to(5, true);
        assert_eq!(edit.selection, 3..5);
        assert!(!edit.reversed);
    }

    #[test]
    fn multiline_normalization_preserves_line_breaks_and_tabs() {
        assert_eq!(normalize_text("a\r\nb\rc\td", true), "a\nb\nc\td");
        assert_eq!(normalize_text("a\r\nb\rc\td", false), "a b c d");
    }

    #[test]
    fn undo_redo_restores_selection_and_drops_redo_on_new_edit() {
        let mut edit = Editing::new("hello".into());
        edit.move_to(0, false);
        edit.move_to(5, true);
        edit.replace(0..5, "world", false, None);
        assert!(edit.history(true));
        assert_eq!(edit.selection, 0..5);
        assert_eq!(edit.text, "hello");
        assert!(edit.history(false));
        assert_eq!(edit.text, "world");
        edit.history(true);
        edit.replace(0..5, "next", false, None);
        assert!(!edit.history(false));
    }

    #[test]
    fn undo_of_implicit_deletion_restores_original_caret() {
        let mut edit = Editing::new("a🦀z".into());
        edit.move_to(5, false);
        edit.replace(1..5, "", false, None);
        assert_eq!(edit.text, "az");
        assert!(edit.history(true));
        assert_eq!(edit.text, "a🦀z");
        assert_eq!(edit.selection, 5..5);
    }

    #[test]
    fn word_and_line_navigation_use_unicode_boundaries() {
        let mut edit = Editing::new("ação 🦀\n日本語 next".into());
        edit.move_to(0, false);
        assert_eq!(edit.word_boundary(true), "ação".len());
        edit.move_to("ação 🦀\n日本語".len(), false);
        assert_eq!(edit.line_boundary(false), "ação 🦀\n".len());
        assert_eq!(edit.line_boundary(true), edit.text.len());
        assert_eq!(edit.word_boundary(false), "ação 🦀\n日本".len());
    }
}
