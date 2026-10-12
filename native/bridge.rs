use gpui::{
    div, prelude::*, px, rgb, size, AnyElement, App, Application, Bounds, Context, SharedString,
    Focusable, Window, WindowBounds, WindowOptions,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    time::Duration,
};
mod text_input;
#[derive(Clone)]
pub struct DesktopWindow {
    pub id: i64,
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub resizable: bool,
    pub minimizable: bool,
}
#[derive(Clone, Debug)]
pub struct DesktopEvent {
    pub window: i64,
    pub node: String,
    pub kind: String,
    pub text: String,
    pub number: f64,
    pub checked: bool,
    pub key: String,
}
pub struct DesktopCommand {
    pub window: i64,
    pub kind: String,
    pub text: String,
    pub width: f64,
    pub height: f64,
}
pub struct DesktopPoll {
    pub window: i64,
    pub changed: bool,
    pub pending: bool,
    pub next_ms: f64,
}
pub struct DesktopTurn {
    pub windows: Vec<DesktopWindow>,
    pub commands: Vec<DesktopCommand>,
    pub polls: Vec<DesktopPoll>,
}
#[derive(Clone, Default, Debug)]
pub struct DesktopTextSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub code: bool,
    pub color: i64,
    pub link: String,
}
#[derive(Clone)]
pub struct DesktopPrimitive {
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub radius: f64,
    pub color: i64,
    pub stroke: i64,
    pub stroke_width: f64,
    pub text: String,
    pub font_size: f64,
}
#[derive(Clone)]
pub struct DesktopNode {
    pub kind: String,
    pub key: String,
    pub text: String,
    pub placeholder: String,
    pub style: Style,
    pub disabled: bool,
    pub read_only: bool,
    pub checked: bool,
    pub multiline: bool,
    pub submit_on_enter: bool,
    pub ratio: f64,
    pub vertical: bool,
    pub source: String,
    pub language: String,
    pub action: String,
    pub tooltip: String,
    pub accepts_keys: bool,
    pub runs: Vec<DesktopTextSpan>,
    pub primitives: Vec<DesktopPrimitive>,
    pub children: Vec<DesktopNode>,
}

struct SplitState {
    ratio: f64,
    external_ratio: f64,
    vertical: bool,
    bounds: Bounds<gpui::Pixels>,
    dragging: bool,
}
impl SplitState {
    fn sync(&mut self, ratio: f64, vertical: bool) {
        let ratio = ratio.clamp(0.1, 0.9);
        if ratio != self.external_ratio {
            self.ratio = ratio;
            self.external_ratio = ratio;
        }
        if self.vertical != vertical {
            self.vertical = vertical;
            self.dragging = false;
        }
    }
    fn drag(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        let origin = if self.vertical {
            self.bounds.origin.y
        } else {
            self.bounds.origin.x
        };
        let extent = if self.vertical {
            self.bounds.size.height
        } else {
            self.bounds.size.width
        };
        let position = if self.vertical {
            position.y
        } else {
            position.x
        };
        let available: f64 = (extent - px(6.)).max(px(1.)).into();
        let offset: f64 = (position - origin - px(3.)).into();
        let ratio = (offset / available).clamp(0.1, 0.9);
        if (ratio - self.ratio).abs() < 0.0001 {
            return false;
        }
        self.ratio = ratio;
        true
    }
}

struct DesktopRoot {
    id: i64,
    // The foreign render callback runs in the driver, outside every App/entity
    // borrow. Native redraws (including caret blinking) reuse this owned tree.
    nodes: Vec<DesktopNode>,
    published_revision: u64,
    acknowledged_edits: HashMap<String, u64>,
    edit_sequence: Rc<Cell<u64>>,
    events: Rc<RefCell<Vec<DesktopEvent>>>,
    failure: Rc<RefCell<Option<String>>>,
    wake: Rc<dyn Fn()>,
    inputs: HashMap<String, gpui::Entity<text_input::TextInput>>,
    focus: HashMap<String, gpui::FocusHandle>,
    focus_order: Vec<String>,
    splits: HashMap<String, Rc<RefCell<SplitState>>>,
    pending_focus: Vec<String>,
    root_focus: gpui::FocusHandle,
    modal: Option<String>,
    modal_previous_focus: Option<gpui::FocusHandle>,
}

fn desktop_event(window: i64, node: String, kind: &str) -> DesktopEvent {
    DesktopEvent {
        window,
        node,
        kind: kind.into(),
        text: String::new(),
        number: 0.,
        checked: false,
        key: String::new(),
    }
}

type EditAcknowledgements = HashMap<(i64, String), u64>;

fn acknowledge_input_batch(acknowledged: &mut EditAcknowledgements, batch: &[DesktopEvent]) {
    for event in batch {
        // Only actual processed changes acknowledge editing. Resize ratios and
        // other numeric event payloads have no input-sequence meaning.
        if event.kind == "change"
            && event.number.is_finite()
            && event.number > 0.
            && event.number <= 9_007_199_254_740_991.
            && event.number.fract() == 0.
        {
            let sequence = event.number as u64;
            let old = acknowledged
                .entry((event.window, event.node.clone()))
                .or_default();
            *old = (*old).max(sequence);
        }
    }
}

fn publication_acknowledgements(
    acknowledged: &EditAcknowledgements,
    window: i64,
) -> HashMap<String, u64> {
    acknowledged
        .iter()
        .filter_map(|((id, key), sequence)| (*id == window).then_some((key.clone(), *sequence)))
        .collect()
}
fn queue_event(events: &Rc<RefCell<Vec<DesktopEvent>>>, event: DesktopEvent, wake: &Rc<dyn Fn()>) {
    let mut events = events.borrow_mut();
    // Pointer drags may produce several positions before the next host turn.
    // Keep the final value without delaying unrelated clicks or submissions.
    if event.kind == "resize" {
        if let Some(previous) = events.last_mut().filter(|previous| {
            previous.kind == "resize"
                && previous.window == event.window
                && previous.node == event.node
        }) {
            *previous = event;
        } else {
            events.push(event);
        }
    } else {
        events.push(event);
    }
    drop(events);
    wake();
}
fn activation(window: i64, node: String, kind: &str, checked: bool, action: &str) -> DesktopEvent {
    let toggle = matches!(kind, "checkbox" | "switch");
    let mut event = desktop_event(
        window,
        node,
        if toggle {
            "change"
        } else if action.is_empty() {
            "click"
        } else {
            "action"
        },
    );
    event.checked = toggle && !checked;
    if !toggle {
        event.text = action.into();
    }
    event
}

fn validate_window(spec: &DesktopWindow) -> Result<(), String> {
    if spec.id <= 0
        || !spec.width.is_finite()
        || !spec.height.is_finite()
        || spec.width <= 0.
        || spec.height <= 0.
        || !(spec.width as f32).is_finite()
        || !(spec.height as f32).is_finite()
    {
        return Err(format!(
            "invalid desktop window specification for {}",
            spec.id
        ));
    }
    Ok(())
}
fn validate_window_batch(specs: &[DesktopWindow], allocated: &HashSet<i64>) -> Result<(), String> {
    let mut ids = allocated.clone();
    for spec in specs {
        validate_window(spec)?;
        if !ids.insert(spec.id) {
            return Err(format!("duplicate desktop window id {}", spec.id));
        }
    }
    Ok(())
}
fn validate_tree(nodes: &[DesktopNode]) -> Result<(), String> {
    fn visit(node: &DesktopNode, keys: &mut HashSet<String>) -> Result<(), String> {
        if node.key.is_empty() || !keys.insert(node.key.clone()) {
            return Err(format!(
                "duplicate or empty desktop node key '{}'",
                node.key
            ));
        }
        let style = &node.style;
        if [
            style.gap,
            style.padding,
            style.radius,
            style.font_size,
            style.width,
            style.height,
            style.grow,
            style.shrink,
        ]
        .into_iter()
        .any(|value| !value.is_finite() || !(value as f32).is_finite())
        {
            return Err(format!("non-finite desktop style on '{}'", node.key));
        }
        if style.grow < 0. || style.shrink < 0. {
            return Err(format!("negative desktop flex factor on '{}'", node.key));
        }
        if matches!(node.kind.as_str(), "split" | "progress") && !node.ratio.is_finite() {
            return Err(format!("non-finite ratio on '{}'", node.key));
        }
        if node.kind == "split" && flattened_count(&node.children) != 2 {
            return Err(format!("split '{}' requires exactly two panes", node.key));
        }
        for primitive in &node.primitives {
            if [
                primitive.x,
                primitive.y,
                primitive.width,
                primitive.height,
                primitive.radius,
                primitive.stroke_width,
                primitive.font_size,
            ]
            .into_iter()
            .any(|value| !value.is_finite() || !(value as f32).is_finite())
            {
                return Err(format!("non-finite canvas primitive on '{}'", node.key));
            }
            if !matches!(
                primitive.kind.as_str(),
                "rect" | "ellipse" | "line" | "text"
            ) {
                return Err(format!("unsupported canvas primitive '{}'", primitive.kind));
            }
        }
        for child in &node.children {
            visit(child, keys)?;
        }
        Ok(())
    }
    let mut keys = HashSet::new();
    for node in nodes {
        visit(node, &mut keys)?;
    }
    Ok(())
}
/// Pure validation can run inside the Zolo prepare phase, before committing it.
pub fn validate_desktop(nodes: Vec<DesktopNode>) -> String {
    validate_tree(&nodes).err().unwrap_or_default()
}
fn flattened_count(nodes: &[DesktopNode]) -> usize {
    nodes
        .iter()
        .map(|node| {
            if node.kind == "fragment" {
                flattened_count(&node.children)
            } else {
                1
            }
        })
        .sum()
}
fn flatten_fragments(nodes: Vec<DesktopNode>, seen: &mut HashSet<String>) -> Vec<DesktopNode> {
    let mut result = Vec::new();
    for node in nodes {
        seen.insert(node.key.clone());
        if node.kind == "fragment" {
            result.extend(flatten_fragments(node.children, seen));
        } else {
            result.push(node);
        }
    }
    result
}
fn resolve_key<'a>(requested: &str, keys: impl Iterator<Item = &'a str>) -> Result<String, String> {
    let keys: Vec<&str> = keys.collect();
    if keys.contains(&requested) {
        return Ok(requested.into());
    }
    let authored = requested.replace('%', "%25").replace('/', "%2F").replace('~', "%7E");
    let matches: Vec<&str> = keys
        .into_iter()
        .filter(|key| key.rsplit('/').next() == Some(authored.as_str()))
        .collect();
    match matches.as_slice() {
        [key] => Ok((*key).into()),
        [] => Err(format!(
            "focus target '{requested}' is not mounted or is disabled"
        )),
        _ => Err(format!(
            "focus target '{requested}' is ambiguous; use its complete component path"
        )),
    }
}
fn last_modal(nodes: &[DesktopNode]) -> Option<String> {
    let mut modal = None;
    for node in nodes {
        if node.kind == "modal" {
            modal = Some(node.key.clone());
        }
        if let Some(child) = last_modal(&node.children) {
            modal = Some(child);
        }
    }
    modal
}
fn belongs_to(key: &str, owner: &str) -> bool {
    key == owner
        || key
            .strip_prefix(owner)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn event_root_key(nodes: &[DesktopNode]) -> Option<&str> {
    nodes.iter().find_map(|node| {
        if node.kind == "fragment" {
            event_root_key(&node.children)
        } else {
            Some(node.key.as_str())
        }
    })
}

/// The encoded component path retains authored ownership through fragments.
/// Route shortcuts to the nearest accepting ancestor of the actual focus,
/// constrained to the top modal boundary when one is mounted.
fn key_handler<'a>(
    nodes: &'a [DesktopNode],
    focused: Option<&str>,
    modal: Option<&str>,
) -> Option<&'a str> {
    let origin = focused.or(modal).or_else(|| event_root_key(nodes))?;
    if modal.is_some_and(|modal| !belongs_to(origin, modal)) {
        return None;
    }
    fn visit<'a>(
        nodes: &'a [DesktopNode],
        origin: &str,
        modal: Option<&str>,
        nearest: &mut Option<&'a str>,
    ) {
        for node in nodes {
            if node.accepts_keys
                && !node.disabled
                && belongs_to(origin, &node.key)
                && modal.is_none_or(|modal| belongs_to(&node.key, modal))
                && nearest.is_none_or(|key| node.key.len() > key.len())
            {
                *nearest = Some(node.key.as_str());
            }
            visit(&node.children, origin, modal, nearest);
        }
    }
    let mut nearest = None;
    visit(nodes, origin, modal, &mut nearest);
    nearest
}

impl DesktopRoot {
    fn focus_handle(&self, key: &str, cx: &App) -> Option<gpui::FocusHandle> {
        self.inputs
            .get(key)
            .map(|input| input.focus_handle(cx))
            .or_else(|| self.focus.get(key).cloned())
    }
    fn apply_focus(
        &mut self,
        requested: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let key = resolve_key(requested, self.focus_order.iter().map(String::as_str))?;
        if self
            .modal
            .as_ref()
            .is_some_and(|modal| !belongs_to(&key, modal))
        {
            return Err("focus target is outside the active modal".into());
        }
        self.focus_handle(&key, cx)
            .ok_or_else(|| format!("focus target '{requested}' disappeared"))?
            .focus(window);
        Ok(())
    }
    fn request_focus(&mut self, requested: String, window: &mut Window, cx: &mut Context<Self>) {
        // State changes in this turn may create the target. Resolve after the
        // corresponding native tree has been rendered and painted.
        let _ = window;
        self.pending_focus.push(requested);
        cx.notify();
    }
    fn drag_split(&mut self, event: &gpui::MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !event.dragging() {
            return;
        }
        for (key, split) in &self.splits {
            let mut split = split.borrow_mut();
            if split.dragging && split.drag(event.position) {
                let mut event = desktop_event(self.id, key.clone(), "resize");
                event.number = split.ratio;
                queue_event(&self.events, event, &self.wake);
                cx.notify();
            }
        }
    }
    fn end_split_drag(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        for split in self.splits.values() {
            split.borrow_mut().dragging = false;
        }
    }
    fn capture_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(modal) = &self.modal {
            if event.keystroke.key == "escape" {
                queue_event(
                    &self.events,
                    desktop_event(self.id, modal.clone(), "dismiss"),
                    &self.wake,
                );
                cx.stop_propagation();
                return;
            }
            if event.keystroke.key == "tab" {
                let handles: Vec<_> = self
                    .focus_order
                    .iter()
                    .filter(|key| belongs_to(key, modal))
                    .filter_map(|key| self.focus_handle(key, cx))
                    .collect();
                if !handles.is_empty() {
                    let focused = handles.iter().position(|handle| handle.is_focused(window));
                    let next = match (focused, event.keystroke.modifiers.shift) {
                        (Some(index), true) => (index + handles.len() - 1) % handles.len(),
                        (Some(index), false) => (index + 1) % handles.len(),
                        (None, true) => handles.len() - 1,
                        (None, false) => 0,
                    };
                    handles[next].focus(window);
                }
                cx.stop_propagation();
                return;
            }
        } else if event.keystroke.key == "tab" {
            if event.keystroke.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            cx.stop_propagation();
            return;
        }
    }
    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // GPUI runs bound native editing actions before raw key bubbling. This
        // listener receives only keys left unconsumed by the focused control.
        let focused = self
            .inputs
            .iter()
            .find_map(|(key, input)| {
                input
                    .focus_handle(cx)
                    .is_focused(window)
                    .then_some(key.as_str())
            })
            .or_else(|| {
                self.focus
                    .iter()
                    .find_map(|(key, handle)| handle.is_focused(window).then_some(key.as_str()))
            });
        let Some(handler) = key_handler(&self.nodes, focused, self.modal.as_deref()) else {
            return;
        };
        let mut record = desktop_event(self.id, handler.into(), "key");
        record.key = event.keystroke.unparse();
        queue_event(&self.events, record, &self.wake);
    }
}

struct Tooltip(String);
impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .rounded_md()
            .bg(rgb(0x27272a))
            .text_color(rgb(0xf4f4f5))
            .border_1()
            .border_color(rgb(0x52525b))
            .child(self.0.clone())
    }
}
struct LocalAssets;
impl gpui::AssetSource for LocalAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes.into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(std::fs::read_dir(path)?
            .filter_map(Result::ok)
            .map(|entry| entry.path().to_string_lossy().to_string().into())
            .collect())
    }
}

fn style_view(mut view: gpui::Stateful<gpui::Div>, style: &Style) -> gpui::Stateful<gpui::Div> {
    view = view
        .gap(px(style.gap.max(0.) as f32))
        .p(px(style.padding.max(0.) as f32))
        .rounded(px(style.radius.max(0.) as f32))
        .text_size(px(style.font_size.max(1.) as f32))
        .min_w(px(0.));
    view.style().flex_grow = Some(style.grow as f32);
    view.style().flex_shrink = Some(style.shrink as f32);
    if style.background >= 0 {
        view = view.bg(rgb(style.background as u32));
    }
    if style.color >= 0 {
        view = view.text_color(rgb(style.color as u32));
    }
    if style.width >= 0. {
        view = view.w(px(style.width as f32));
    }
    if style.height >= 0. {
        view = view.h(px(style.height as f32));
    }
    view
}
#[derive(Clone, Copy)]
enum ParentLayout {
    Row,
    Column,
    Panel,
}

fn desktop_element(
    node: DesktopNode,
    parent_layout: ParentLayout,
    window: &mut Window,
    cx: &mut Context<DesktopRoot>,
    root: &mut DesktopRoot,
    seen: &mut HashSet<String>,
) -> AnyElement {
    let key = node.key.clone();
    seen.insert(key.clone());
    let mut view = style_view(
        div().id(SharedString::from(key.clone())).flex(),
        &node.style,
    );
    #[cfg(all(test, feature = "test-support"))]
    {
        view = view.debug_selector(|| key.clone());
    }
    // Fill only an automatic size along the parent's flex axis. flex_1 sets a
    // zero basis, so applying it to an explicit height/width discards the
    // authored size and can move following controls.
    let automatic_main_size = if matches!(parent_layout, ParentLayout::Row) {
        node.style.width < 0.
    } else {
        node.style.height < 0.
    };
    view = if node.kind == "row" {
        view.flex_row()
    } else {
        view.flex_col()
    };
    if node.kind == "column" && node.style.height < 0. {
        if matches!(parent_layout, ParentLayout::Panel) {
            view = view.flex_grow().min_h(px(0.));
        } else if node.style.grow > 0. {
            view = view.min_h(px(0.));
        }
    }
    if node.kind == "scroll" {
        view = view.overflow_y_scroll().min_h(px(0.));
        if automatic_main_size {
            view = view.flex_1();
        }
    }
    if node.kind == "spacer" {
        view = view.flex_grow();
    }
    if node.style.grow > 0. {
        view.style().flex_grow = Some(node.style.grow as f32);
    }
    view.style().flex_shrink = Some(node.style.shrink as f32);
    if node.kind == "separator" {
        view = view.h(px(1.)).w_full().bg(rgb(0x3f3f46));
    }
    if !node.tooltip.is_empty() {
        let text = node.tooltip.clone();
        view = view.tooltip(move |_, cx| cx.new(|_| Tooltip(text.clone())).into());
    }
    if node.disabled {
        view = view.opacity(0.45);
    }
    if node.kind == "input" {
        let props = text_input::InputProps {
            value: node.text,
            placeholder: node.placeholder,
            multiline: node.multiline,
            disabled: node.disabled,
            read_only: node.read_only,
            submit_on_enter: node.submit_on_enter,
        };
        let height = if node.style.height > 0. {
            (node.style.height - node.style.padding.max(0.) * 2.).max(1.)
        } else {
            0.
        };
        let input = if let Some(input) = root.inputs.get(&key) {
            input.update(cx, |input, cx| {
                input.sync_revision(
                    props,
                    root.published_revision,
                    root.acknowledged_edits.get(&key).copied().unwrap_or(0),
                    cx,
                );
                input.set_viewport_height(height as f32, cx);
            });
            input.clone()
        } else {
            let events = root.events.clone();
            let wake = root.wake.clone();
            let id = root.id;
            let node_key = key.clone();
            let emit: Rc<dyn Fn(text_input::InputEvent)> = Rc::new(move |event| {
                let mut record = desktop_event(id, node_key.clone(), &event.kind);
                record.text = event.text;
                if event.kind == "change" {
                    record.number = event.edit_sequence as f64;
                }
                queue_event(&events, record, &wake);
            });
            let input = cx.new(|cx| {
                let mut input = text_input::TextInput::with_sequence(
                    props.clone(),
                    window,
                    cx,
                    emit,
                    root.edit_sequence.clone(),
                );
                input.sync_revision(
                    props,
                    root.published_revision,
                    root.acknowledged_edits.get(&key).copied().unwrap_or(0),
                    cx,
                );
                input.set_viewport_height(height as f32, cx);
                input
            });
            root.inputs.insert(key.clone(), input.clone());
            input
        };
        let modal_allows = root
            .modal
            .as_ref()
            .is_none_or(|modal| belongs_to(&key, modal));
        input
            .focus_handle(cx)
            .tab_stop(!node.disabled && modal_allows);
        if !node.disabled {
            root.focus_order.push(key.clone());
        }
        return view.child(input).into_any_element();
    }
    if node.kind == "split" {
        let split = root
            .splits
            .entry(key.clone())
            .or_insert_with(|| {
                Rc::new(RefCell::new(SplitState {
                    ratio: node.ratio.clamp(0.1, 0.9),
                    external_ratio: node.ratio.clamp(0.1, 0.9),
                    vertical: node.vertical,
                    bounds: Bounds::default(),
                    dragging: false,
                }))
            })
            .clone();
        split.borrow_mut().sync(node.ratio, node.vertical);
        let interactive = !node.disabled
            && !node.read_only
            && root
                .modal
                .as_ref()
                .is_none_or(|modal| belongs_to(&key, modal));
        if !interactive {
            split.borrow_mut().dragging = false;
        }
        let ratio = split.borrow().ratio;
        // The divider owns its six pixels of spacing; ordinary layout gaps
        // would otherwise distort the ratio and pointer geometry.
        view = view
            .relative()
            .min_h(px(0.))
            .gap(px(0.))
            .overflow_hidden();
        if automatic_main_size {
            view = view.flex_1();
        }
        if node.style.grow > 0. {
            view.style().flex_grow = Some(node.style.grow as f32);
        }
        view.style().flex_shrink = Some(node.style.shrink as f32);
        view = if node.vertical {
            view.flex_col()
        } else {
            view.flex_row()
        };
        let bounds_state = split.clone();
        view = view.child(
            gpui::canvas(
                move |bounds, _, _| bounds_state.borrow_mut().bounds = bounds,
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
        let mut panes = flatten_fragments(node.children, seen).into_iter();
        let first = panes.next().expect("validated first split pane");
        let second = panes.next().expect("validated second split pane");
        let mut first_view = div()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_hidden();
        first_view.style().flex_basis = Some(px(0.).into());
        first_view.style().flex_grow = Some(ratio as f32);
        let mut second_view = div()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_hidden();
        second_view.style().flex_basis = Some(px(0.).into());
        second_view.style().flex_grow = Some((1. - ratio) as f32);
        first_view = first_view.child(desktop_element(first, ParentLayout::Panel, window, cx, root, seen));
        second_view = second_view.child(desktop_element(second, ParentLayout::Panel, window, cx, root, seen));
        let mut handle = div()
            .id(SharedString::from(format!("{key}/__native_split_handle")))
            .flex_shrink_0()
            .bg(rgb(0x3f3f46))
            .hover(|style| style.bg(rgb(0x71717a)));
        handle = if node.vertical {
            handle
                .h(px(6.))
                .w_full()
                .cursor(gpui::CursorStyle::ResizeUpDown)
        } else {
            handle
                .w(px(6.))
                .h_full()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
        };
        if interactive {
            handle = handle.on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                split.borrow_mut().dragging = true;
                cx.stop_propagation();
            });
        }
        return view
            .child(first_view)
            .child(handle)
            .child(second_view)
            .into_any_element();
    }
    if node.kind == "image" {
        let source = std::path::PathBuf::from(&node.source);
        let caption = if node.text.is_empty() {
            "Image unavailable".to_string()
        } else {
            node.text.clone()
        };
        let width = if node.style.width > 0. {
            node.style.width
        } else {
            320.
        };
        let height = if node.style.height > 0. {
            node.style.height
        } else {
            180.
        };
        use gpui::StyledImage;
        return view
            .child(
                gpui::img(source)
                    .id(SharedString::from(format!("{key}/__native_image")))
                    .w(px(width as f32))
                    .h(px(height as f32))
                    .with_fallback(move || {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0xa1a1aa))
                            .child(caption.clone())
                            .into_any_element()
                    }),
            )
            .into_any_element();
    }
    if node.kind == "icon" {
        let width = if node.style.width > 0. {
            node.style.width
        } else {
            20.
        };
        let height = if node.style.height > 0. {
            node.style.height
        } else {
            20.
        };
        return view
            .child(
                gpui::svg()
                    .path(node.source)
                    .w(px(width as f32))
                    .h(px(height as f32))
                    .text_color(rgb(if node.style.color >= 0 {
                        node.style.color as u32
                    } else {
                        0xf4f4f5
                    })),
            )
            .into_any_element();
    }
    if node.kind == "canvas" {
        let primitives = node.primitives;
        let width = if node.style.width > 0. {
            node.style.width
        } else {
            320.
        };
        let height = if node.style.height > 0. {
            node.style.height
        } else {
            180.
        };
        return view
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| paint_primitives(&primitives, bounds, window, cx),
                )
                .w(px(width as f32))
                .h(px(height as f32)),
            )
            .into_any_element();
    }
    if node.kind == "progress" {
        return view
            .h(px(if node.style.height > 0. {
                node.style.height as f32
            } else {
                6.
            }))
            .w_full()
            .rounded_full()
            .bg(rgb(0x3f3f46))
            .child(
                div()
                    .h_full()
                    .w(gpui::relative(node.ratio.clamp(0., 1.) as f32))
                    .rounded_full()
                    .bg(rgb(if node.style.color >= 0 {
                        node.style.color as u32
                    } else {
                        0x60a5fa
                    })),
            )
            .into_any_element();
    }
    if node.kind == "rich_text" {
        return view
            .child(rich_text(
                &key,
                &key,
                node.runs,
                node.disabled,
                root,
                window,
            ))
            .into_any_element();
    }
    if node.kind == "markdown" {
        let blocks = markdown_blocks(&node.text);
        for (index, block) in blocks.into_iter().enumerate() {
            let id = format!("{key}/__native_markdown_{index}");
            if block.kind == "rule" {
                view = view.child(div().h(px(1.)).w_full().my_2().bg(rgb(0x52525b)));
            } else if block.kind == "code" {
                view = view.child(code_block(
                    &id,
                    &key,
                    block
                        .runs
                        .iter()
                        .map(|run| run.text.as_str())
                        .collect::<String>(),
                    block.language,
                    node.disabled,
                    root,
                ));
            } else {
                let mut block_view = div().w_full().mb_2();
                if block.heading > 0 {
                    block_view = block_view.text_size(px(node.style.font_size.max(1.) as f32
                        * (1. + (7 - block.heading) as f32 * 0.12)));
                }
                if block.kind == "quote" {
                    block_view = block_view.pl_3().border_l_2().border_color(rgb(0x71717a));
                }
                view = view.child(block_view.child(rich_text(
                    &id,
                    &key,
                    block.runs,
                    node.disabled,
                    root,
                    window,
                )));
            }
        }
        return view.into_any_element();
    }
    if node.kind == "code" {
        return view
            .child(code_block(
                &format!("{key}/__native_code"),
                &key,
                node.text,
                node.language,
                node.disabled,
                root,
            ))
            .into_any_element();
    }
    if node.kind == "checkbox" {
        let mut check = div()
            .size(px(17.))
            .flex_shrink_0()
            .rounded_sm()
            .border_1()
            .border_color(rgb(0xa1a1aa))
            .items_center()
            .justify_center();
        if node.checked {
            check = check.bg(rgb(0x2563eb)).child("✓");
        }
        view = view.flex_row().items_center().child(check);
    }
    if node.kind == "switch" {
        let thumb = div().size(px(14.)).rounded_full().bg(rgb(0xffffff));
        view = view.flex_row().items_center().child(
            div()
                .w(px(30.))
                .h(px(18.))
                .p(px(2.))
                .flex()
                .rounded_full()
                .bg(rgb(if node.checked { 0x2563eb } else { 0x52525b }))
                .when(node.checked, |view| view.justify_end())
                .child(thumb),
        );
    }
    if node.kind == "badge" {
        view = view
            .px_2()
            .py_1()
            .rounded_full()
            .bg(rgb(if node.style.background >= 0 {
                node.style.background as u32
            } else {
                0x3f3f46
            }));
    }
    if node.kind == "menu" {
        view = view
            .p_2()
            .border_1()
            .border_color(rgb(0x52525b))
            .rounded_md()
            .bg(rgb(0x27272a));
    }
    if !node.text.is_empty() {
        view = view.child(node.text);
    }
    let toggle = matches!(node.kind.as_str(), "checkbox" | "switch");
    let modal_allows = root
        .modal
        .as_ref()
        .is_none_or(|modal| belongs_to(&key, modal));
    if !node.disabled
        && !(toggle && node.read_only)
        && modal_allows
        && matches!(
            node.kind.as_str(),
            "button" | "checkbox" | "switch" | "tab" | "select_item"
        )
    {
        let focus = root
            .focus
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
            .clone();
        root.focus_order.push(key.clone());
        view = view
            .track_focus(&focus)
            .cursor_pointer()
            .focus(|style| style.border_1().border_color(rgb(0x60a5fa)));
        let events = root.events.clone();
        let wake = root.wake.clone();
        let id = root.id;
        let node_key = key.clone();
        let checked = node.checked;
        let kind = node.kind.clone();
        let action = node.action.clone();
        let click_focus = focus.clone();
        view = view.on_click(move |_, window, cx| {
            click_focus.focus(window);
            queue_event(
                &events,
                activation(id, node_key.clone(), &kind, checked, &action),
                &wake,
            );
            cx.stop_propagation();
        });
        view = view.on_key_down(move |event, window, cx| {
            // GPUI's on_click synthesizes one keyboard click on Enter/Space
            // key-up. Consume the matching key-down without another activation
            // so it cannot also trigger an authored ancestor shortcut. An
            // ancestor control must not consume keys for its focused children.
            if focus.is_focused(window)
                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                && !event.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
            }
        });
    }
    // Scroll the intrinsic content as a whole. Letting the viewport distribute
    // its fixed height among individual cards can shrink them below their
    // Markdown/code contents and paint those contents over the next card.
    let children = flatten_fragments(node.children, seen);
    if node.kind == "scroll" {
        let mut content = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w_full()
            .gap(px(node.style.gap.max(0.) as f32));
        for child in children {
            content = content.child(desktop_element(child, ParentLayout::Column, window, cx, root, seen));
        }
        view = view.child(content);
    } else {
        for child in children {
            view = view.child(desktop_element(child, if node.kind == "row" { ParentLayout::Row } else { ParentLayout::Column }, window, cx, root, seen));
        }
    }
    if node.kind == "modal" {
        let events = root.events.clone();
        let wake = root.wake.clone();
        let id = root.id;
        let modal_key = key.clone();
        let panel = view
            .w(if node.style.width > 0. {
                px(node.style.width as f32)
            } else {
                px(480.)
            })
            .max_w(gpui::relative(0.9))
            .p_4()
            .rounded_lg()
            .bg(rgb(if node.style.background >= 0 {
                node.style.background as u32
            } else {
                0x27272a
            }))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
        return div()
            .id(SharedString::from(format!("{key}/__native_overlay")))
            .absolute()
            .inset_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000099))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                queue_event(
                    &events,
                    desktop_event(id, modal_key.clone(), "dismiss"),
                    &wake,
                );
                cx.stop_propagation();
            })
            .child(panel)
            .into_any_element();
    }
    view.into_any_element()
}

impl Render for DesktopRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nodes = self.nodes.clone();
        let next_modal = last_modal(&nodes);
        let modal_changed = next_modal != self.modal;
        if modal_changed && next_modal.is_some() && self.modal.is_none() {
            self.modal_previous_focus = window.focused(cx);
        }
        self.modal = next_modal;
        self.focus_order.clear();
        let mut seen = HashSet::new();
        let mut view = div()
            .relative()
            .size_full()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5))
            .flex()
            .flex_col()
            .track_focus(&self.root_focus)
            .capture_key_down(cx.listener(Self::capture_key_down))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_move(cx.listener(Self::drag_split))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::end_split_drag))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::end_split_drag));
        for node in flatten_fragments(nodes, &mut seen) {
            view = view.child(desktop_element(node, ParentLayout::Panel, window, cx, self, &mut seen));
        }
        self.inputs.retain(|key, _| seen.contains(key));
        self.focus.retain(|key, _| seen.contains(key));
        self.splits.retain(|key, _| seen.contains(key));
        if !self.pending_focus.is_empty() || modal_changed {
            let requested = std::mem::take(&mut self.pending_focus);
            cx.on_next_frame(window, move |root, window, cx| {
                if modal_changed {
                    if let Some(modal) = &root.modal {
                        if let Some(handle) = root
                            .focus_order
                            .iter()
                            .filter(|key| belongs_to(key, modal))
                            .find_map(|key| root.focus_handle(key, cx))
                        {
                            handle.focus(window);
                        } else {
                            root.root_focus.focus(window);
                        }
                    } else if let Some(handle) = root.modal_previous_focus.take() {
                        handle.focus(window);
                    }
                }
                for requested in requested {
                    if let Err(error) = root.apply_focus(&requested, window, cx) {
                        root.failure.borrow_mut().get_or_insert(error);
                        (root.wake)();
                        break;
                    }
                }
            });
        }
        view
    }
}

fn rich_text(
    key: &str,
    owner: &str,
    spans: Vec<DesktopTextSpan>,
    disabled: bool,
    root: &DesktopRoot,
    window: &Window,
) -> AnyElement {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&span.text);
        let mut font = window.text_style().font();
        if span.bold {
            font.weight = gpui::FontWeight::BOLD;
        }
        if span.italic {
            font.style = gpui::FontStyle::Italic;
        }
        if span.code {
            font.family = code_font().into();
        }
        let color = rgb(if span.color >= 0 {
            span.color as u32
        } else if !span.link.is_empty() {
            0x60a5fa
        } else {
            0xf4f4f5
        })
        .into();
        runs.push(gpui::TextRun {
            len: span.text.len(),
            font,
            color,
            background_color: span.code.then(|| rgb(0x3f3f46).into()),
            underline: (span.underline || !span.link.is_empty()).then_some(gpui::UnderlineStyle {
                color: Some(color),
                thickness: px(1.),
                wavy: false,
            }),
            strikethrough: None,
        });
        if !span.link.is_empty() {
            links.push((start..text.len(), span.link));
        }
    }
    let styled = gpui::StyledText::new(text).with_runs(runs);
    if links.is_empty() || disabled {
        return styled.into_any_element();
    }
    let ranges = links.iter().map(|(range, _)| range.clone()).collect();
    let destinations: Vec<String> = links
        .into_iter()
        .map(|(_, destination)| destination)
        .collect();
    let events = root.events.clone();
    let wake = root.wake.clone();
    let id = root.id;
    let element_id = SharedString::from(format!("{key}/__native_links"));
    let key = owner.to_string();
    gpui::InteractiveText::new(element_id, styled)
        .on_click(ranges, move |index, _, cx| {
            let mut event = desktop_event(id, key.clone(), "link");
            event.text = destinations[index].clone();
            queue_event(&events, event, &wake);
            cx.stop_propagation();
        })
        .into_any_element()
}
fn code_font() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "monospace"
    }
}
fn code_block(
    id: &str,
    node_key: &str,
    code: String,
    language: String,
    disabled: bool,
    root: &DesktopRoot,
) -> AnyElement {
    let events = root.events.clone();
    let wake = root.wake.clone();
    let window_id = root.id;
    let key = node_key.to_string();
    let copy = code.clone();
    let header = div()
        .flex()
        .flex_row()
        .justify_between()
        .px_3()
        .py_1()
        .text_sm()
        .child(language)
        .child(
            div()
                .id(SharedString::from(format!("{id}/copy")))
                .child("Copy")
                .when(!disabled, |view| {
                    view.cursor_pointer().on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
                        let mut event = desktop_event(window_id, key.clone(), "action");
                        event.text = "copy".into();
                        queue_event(&events, event, &wake);
                        cx.stop_propagation();
                    })
                }),
        );
    div()
        .id(SharedString::from(id.to_string()))
        .w_full()
        .rounded_md()
        .bg(rgb(0x09090b))
        .flex()
        .flex_col()
        .child(header)
        .child(
            div()
                .id(SharedString::from(format!("{id}/content")))
                .overflow_x_scroll()
                .p_3()
                .font_family(code_font())
                .whitespace_nowrap()
                .child(code),
        )
        .into_any_element()
}

#[derive(Default, Debug)]
struct MarkdownBlock {
    kind: String,
    runs: Vec<DesktopTextSpan>,
    heading: usize,
    language: String,
}
fn markdown_blocks(markdown: &str) -> Vec<MarkdownBlock> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
    let mut blocks = Vec::new();
    let mut current = MarkdownBlock::default();
    let mut bold = 0;
    let mut italic = 0;
    let mut links = Vec::<String>::new();
    let mut list = Vec::<Option<u64>>::new();
    let mut quote = 0;
    fn flush(current: &mut MarkdownBlock, blocks: &mut Vec<MarkdownBlock>) {
        if !current.runs.is_empty() || current.kind == "rule" {
            blocks.push(std::mem::take(current));
        }
    }
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        match event {
            Event::Start(Tag::Paragraph) => {
                if current.kind.is_empty() {
                    current.kind = if quote > 0 { "quote" } else { "text" }.into();
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut current, &mut blocks);
                current.heading = level as usize;
                current.kind = "heading".into();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut current, &mut blocks);
                current.kind = "code".into();
                if let CodeBlockKind::Fenced(language) = kind {
                    current.language = language.into_string();
                }
            }
            Event::Start(Tag::Strong) => bold += 1,
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::Start(Tag::Link { dest_url, .. }) => links.push(dest_url.into_string()),
            Event::Start(Tag::Image { dest_url, .. }) => links.push(dest_url.into_string()),
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut current, &mut blocks);
                quote += 1;
                current.kind = "quote".into();
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut current, &mut blocks);
                list.push(start);
            }
            Event::Start(Tag::Item) => {
                flush(&mut current, &mut blocks);
                current.kind = "text".into();
                let indent = "  ".repeat(list.len().saturating_sub(1));
                let prefix = if let Some(Some(number)) = list.last_mut() {
                    let prefix = format!("{number}. ");
                    *number += 1;
                    prefix
                } else {
                    "• ".into()
                };
                current.runs.push(DesktopTextSpan {
                    text: indent + &prefix,
                    color: -1,
                    ..Default::default()
                });
            }
            Event::Start(Tag::TableCell) => {
                if !current.runs.is_empty() {
                    current.runs.push(DesktopTextSpan {
                        text: " | ".into(),
                        color: -1,
                        ..Default::default()
                    });
                }
            }
            Event::End(TagEnd::Strong) => bold -= 1,
            Event::End(TagEnd::Emphasis) => italic -= 1,
            Event::End(TagEnd::Link | TagEnd::Image) => {
                links.pop();
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut current, &mut blocks);
                list.pop();
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut current, &mut blocks);
                quote -= 1;
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::TableRow
                | TagEnd::TableHead,
            ) => flush(&mut current, &mut blocks),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                current.runs.push(DesktopTextSpan {
                    text: text.into_string(),
                    bold: bold > 0 || current.heading > 0,
                    italic: italic > 0,
                    code: current.kind == "code",
                    color: -1,
                    link: links.last().cloned().unwrap_or_default(),
                    ..Default::default()
                })
            }
            Event::Code(text) => current.runs.push(DesktopTextSpan {
                text: text.into_string(),
                bold: bold > 0,
                italic: italic > 0,
                code: true,
                color: -1,
                link: links.last().cloned().unwrap_or_default(),
                ..Default::default()
            }),
            Event::SoftBreak | Event::HardBreak => current.runs.push(DesktopTextSpan {
                text: "\n".into(),
                color: -1,
                ..Default::default()
            }),
            Event::Rule => {
                flush(&mut current, &mut blocks);
                current.kind = "rule".into();
                flush(&mut current, &mut blocks);
            }
            Event::TaskListMarker(checked) => current.runs.push(DesktopTextSpan {
                text: if checked { "[x] " } else { "[ ] " }.into(),
                color: -1,
                ..Default::default()
            }),
            _ => {}
        }
    }
    flush(&mut current, &mut blocks);
    blocks
}

fn paint_primitives(
    primitives: &[DesktopPrimitive],
    bounds: Bounds<gpui::Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        for primitive in primitives {
            let origin =
                bounds.origin + gpui::point(px(primitive.x as f32), px(primitive.y as f32));
            let dimensions = size(
                px(primitive.width.max(0.) as f32),
                px(primitive.height.max(0.) as f32),
            );
            let background: gpui::Hsla = if primitive.color >= 0 {
                rgb(primitive.color as u32).into()
            } else {
                gpui::rgba(0).into()
            };
            let border: gpui::Hsla = if primitive.stroke >= 0 {
                rgb(primitive.stroke as u32).into()
            } else {
                gpui::rgba(0).into()
            };
            match primitive.kind.as_str() {
                "rect" => window.paint_quad(gpui::quad(
                    Bounds::new(origin, dimensions),
                    px(primitive.radius.max(0.) as f32),
                    background,
                    px(primitive.stroke_width.max(0.) as f32),
                    border,
                    gpui::BorderStyle::Solid,
                )),
                "ellipse" => {
                    let rx = dimensions.width / 2.;
                    let ry = dimensions.height / 2.;
                    let center = origin + gpui::point(rx, ry);
                    for (stroke, color) in [(false, primitive.color), (true, primitive.stroke)] {
                        if color < 0 || (stroke && primitive.stroke_width <= 0.) {
                            continue;
                        }
                        let mut path = if stroke {
                            gpui::PathBuilder::stroke(px(primitive.stroke_width as f32))
                        } else {
                            gpui::PathBuilder::fill()
                        };
                        path.move_to(center + gpui::point(rx, px(0.)));
                        path.arc_to(
                            gpui::point(rx, ry),
                            px(0.),
                            false,
                            true,
                            center - gpui::point(rx, px(0.)),
                        );
                        path.arc_to(
                            gpui::point(rx, ry),
                            px(0.),
                            false,
                            true,
                            center + gpui::point(rx, px(0.)),
                        );
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(color as u32));
                        }
                    }
                }
                "line" => {
                    let mut path =
                        gpui::PathBuilder::stroke(px(primitive.stroke_width.max(1.) as f32));
                    path.move_to(origin);
                    path.line_to(
                        origin
                            + gpui::point(px(primitive.width as f32), px(primitive.height as f32)),
                    );
                    if let Ok(path) = path.build() {
                        window.paint_path(
                            path,
                            rgb(if primitive.stroke >= 0 {
                                primitive.stroke as u32
                            } else if primitive.color >= 0 {
                                primitive.color as u32
                            } else {
                                0xf4f4f5
                            }),
                        );
                    }
                }
                "text" => {
                    let style = window.text_style();
                    let font_size = px(primitive.font_size.max(1.) as f32);
                    let run = gpui::TextRun {
                        len: primitive.text.len(),
                        font: style.font(),
                        color: if primitive.color >= 0 {
                            rgb(primitive.color as u32).into()
                        } else {
                            style.color
                        },
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window.text_system().shape_line(
                        primitive.text.clone().into(),
                        font_size,
                        &[run],
                        None,
                    );
                    let _ = line.paint(origin, font_size * 1.4, window, cx);
                }
                _ => {}
            }
        }
    });
}

fn render_desktop_tree(
    id: i64,
    render: &Rc<dyn Fn(i64) -> Vec<DesktopNode>>,
) -> Result<Vec<DesktopNode>, String> {
    let nodes = catch_unwind(AssertUnwindSafe(|| render(id))).map_err(panic_message)?;
    validate_tree(&nodes)?;
    Ok(nodes)
}

/// This invocation owns windows and native entities. Foreign rendering, event
/// dispatch and task driving all run outside mutable native App/entity borrows.
pub fn desktop(
    initial: Vec<DesktopWindow>,
    updates: bool,
    render: impl Fn(i64) -> Vec<DesktopNode> + 'static,
    dispatch: impl Fn(Vec<DesktopEvent>) + 'static,
    advance: impl Fn() -> DesktopTurn + 'static,
    close: impl Fn(i64) -> bool + 'static,
) -> Result<(), String> {
    if initial.is_empty() {
        return Err("an application needs at least one window".into());
    }
    validate_window_batch(&initial, &HashSet::new())?;
    let render: Rc<dyn Fn(i64) -> Vec<DesktopNode>> = Rc::new(render);
    // Prepare all initial trees before opening any physical window.
    let mut prepared = Vec::new();
    for spec in initial {
        let nodes = render_desktop_tree(spec.id, &render)?;
        prepared.push((spec, nodes));
    }
    let failure = Rc::new(RefCell::new(None::<String>));
    let result = failure.clone();
    Application::new()
        .with_assets(LocalAssets)
        .run(move |cx: &mut App| {
            text_input::init(cx);
            let dispatch = Rc::new(dispatch);
            let advance = Rc::new(advance);
            let close = Rc::new(close);
            let events = Rc::new(RefCell::new(Vec::<DesktopEvent>::new()));
            let acknowledged_edits = Rc::new(RefCell::new(EditAcknowledgements::new()));
            let windows = Rc::new(RefCell::new(
                HashMap::<i64, gpui::WindowHandle<DesktopRoot>>::new(),
            ));
            let allocated = Rc::new(RefCell::new(HashSet::<i64>::new()));
            let closing = Rc::new(RefCell::new(HashSet::<i64>::new()));
            let executor = cx.foreground_executor().clone();
            let app = cx.to_async();
            let running = Rc::new(Cell::new(false));
            let wake_slot = Rc::new(RefCell::new(None::<std::rc::Weak<dyn Fn()>>));
            let wake: Rc<dyn Fn()> = {
                let running = running.clone();
                let events = events.clone();
                let acknowledged_edits = acknowledged_edits.clone();
                let windows = windows.clone();
                let allocated = allocated.clone();
                let closing = closing.clone();
                let failure = failure.clone();
                let render = render.clone();
                let wake_slot = wake_slot.clone();
                Rc::new(move || {
                    if running.replace(true) {
                        return;
                    }
                    let running = running.clone();
                    let events = events.clone();
                    let acknowledged_edits = acknowledged_edits.clone();
                    let windows = windows.clone();
                    let allocated = allocated.clone();
                    let closing = closing.clone();
                    let failure = failure.clone();
                    let render = render.clone();
                    let dispatch = dispatch.clone();
                    let advance = advance.clone();
                    let close = close.clone();
                    let mut app = app.clone();
                    let wake_slot = wake_slot.clone();
                    executor
                        .spawn(async move {
                            loop {
                                let live = app
                                    .update(|cx| {
                                        cx.windows()
                                            .into_iter()
                                            .map(|window| window.window_id())
                                            .collect::<HashSet<_>>()
                                    })
                                    .unwrap_or_default();
                                for (id, handle) in windows.borrow().iter() {
                                    if !live.contains(&handle.window_id()) {
                                        closing.borrow_mut().insert(*id);
                                    }
                                }
                                if failure.borrow().is_some() {
                                    closing
                                        .borrow_mut()
                                        .extend(windows.borrow().keys().copied());
                                }
                                let closing_ids: Vec<i64> =
                                    closing.borrow().iter().copied().collect();
                                let mut draining = false;
                                for id in closing_ids {
                                    let done = match catch_unwind(AssertUnwindSafe(|| close(id))) {
                                        Ok(pending) => !pending,
                                        Err(panic) => {
                                            failure
                                                .borrow_mut()
                                                .get_or_insert_with(|| panic_message(panic));
                                            true
                                        }
                                    };
                                    if done {
                                        windows.borrow_mut().remove(&id);
                                        closing.borrow_mut().remove(&id);
                                        acknowledged_edits
                                            .borrow_mut()
                                            .retain(|(window, _), _| *window != id);
                                    } else {
                                        draining = true;
                                    }
                                }
                                let batch = std::mem::take(&mut *events.borrow_mut());
                                if !batch.is_empty() {
                                    let mut batch_acknowledgements = EditAcknowledgements::new();
                                    acknowledge_input_batch(&mut batch_acknowledgements, &batch);
                                    match catch_unwind(AssertUnwindSafe(|| dispatch(batch))) {
                                        Ok(()) => {
                                            let mut acknowledged = acknowledged_edits.borrow_mut();
                                            for (key, sequence) in batch_acknowledgements {
                                                let old = acknowledged.entry(key).or_default();
                                                *old = (*old).max(sequence);
                                            }
                                        }
                                        Err(panic) => {
                                            failure
                                                .borrow_mut()
                                                .get_or_insert_with(|| panic_message(panic));
                                        }
                                    }
                                }
                                let turn = match catch_unwind(AssertUnwindSafe(|| advance())) {
                                    Ok(turn) => turn,
                                    Err(panic) => {
                                        failure
                                            .borrow_mut()
                                            .get_or_insert_with(|| panic_message(panic));
                                        DesktopTurn {
                                            windows: vec![],
                                            commands: vec![],
                                            polls: vec![],
                                        }
                                    }
                                };
                                if failure.borrow().is_none() {
                                    let valid =
                                        validate_window_batch(&turn.windows, &allocated.borrow());
                                    match valid {
                                        Err(error) => {
                                            failure.borrow_mut().get_or_insert(error);
                                        }
                                        Ok(()) => {
                                            let mut new_windows = Vec::new();
                                            for spec in turn.windows {
                                                match render_desktop_tree(spec.id, &render) {
                                                    Ok(nodes) => new_windows.push((spec, nodes)),
                                                    Err(error) => {
                                                        failure.borrow_mut().get_or_insert(error);
                                                        break;
                                                    }
                                                }
                                            }
                                            if failure.borrow().is_none() {
                                                for (spec, nodes) in new_windows {
                                                    let events = events.clone();
                                                    let failure = failure.clone();
                                                    let wake = wake_slot
                                                        .borrow()
                                                        .as_ref()
                                                        .and_then(std::rc::Weak::upgrade)
                                                        .expect("active foreground driver");
                                                    let opened = app.update(|cx| {
                                                        desktop_open(
                                                            spec,
                                                            nodes,
                                                            events,
                                                            failure.clone(),
                                                            wake,
                                                            cx,
                                                        )
                                                    });
                                                    match opened {
                                                        Ok(Ok((id, handle))) => {
                                                            allocated.borrow_mut().insert(id);
                                                            windows.borrow_mut().insert(id, handle);
                                                        }
                                                        Ok(Err(error)) => {
                                                            failure
                                                                .borrow_mut()
                                                                .get_or_insert(error);
                                                        }
                                                        Err(error) => {
                                                            failure
                                                                .borrow_mut()
                                                                .get_or_insert_with(|| {
                                                                    error.to_string()
                                                                });
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                for command in turn.commands {
                                    let handle = windows.borrow().get(&command.window).copied();
                                    if let Some(handle) = handle {
                                        match command.kind.as_str() {
                                            "close" => {
                                                closing.borrow_mut().insert(command.window);
                                                let _ = handle.update(&mut app, |_, window, _| {
                                                    window.remove_window()
                                                });
                                            }
                                            "title" => {
                                                let _ = handle.update(&mut app, |_, window, _| {
                                                    window.set_window_title(&command.text)
                                                });
                                            }
                                            "resize"
                                                if command.width.is_finite()
                                                    && command.height.is_finite()
                                                    && command.width > 0.
                                                    && command.height > 0.
                                                    && (command.width as f32).is_finite()
                                                    && (command.height as f32).is_finite() =>
                                            {
                                                let _ = handle.update(&mut app, |_, window, _| {
                                                    window.resize(size(
                                                        px(command.width as f32),
                                                        px(command.height as f32),
                                                    ))
                                                });
                                            }
                                            "focus" => {
                                                let _ =
                                                    handle.update(&mut app, |root, window, cx| {
                                                        root.request_focus(command.text, window, cx)
                                                    });
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                let mut pending = draining
                                    || !closing.borrow().is_empty()
                                    || failure.borrow().is_some();
                                let mut delay: f64 = 16.;
                                for poll in turn.polls {
                                    if closing.borrow().contains(&poll.window) {
                                        continue;
                                    }
                                    let handle = windows.borrow().get(&poll.window).copied();
                                    if poll.changed && failure.borrow().is_none() {
                                        if let Some(handle) = handle {
                                            // Freeze the exact processed batch state alongside
                                            // the host tree, before any later native edit.
                                            let publication_edits = publication_acknowledgements(
                                                &acknowledged_edits.borrow(),
                                                poll.window,
                                            );
                                            match render_desktop_tree(poll.window, &render) {
                                                Ok(nodes) => {
                                                    let _ =
                                                        handle.update(&mut app, |root, _, cx| {
                                                            root.nodes = nodes;
                                                            root.acknowledged_edits =
                                                                publication_edits;
                                                            root.published_revision = root
                                                                .published_revision
                                                                .wrapping_add(1);
                                                            cx.notify();
                                                        });
                                                }
                                                Err(error) => {
                                                    failure.borrow_mut().get_or_insert(error);
                                                    pending = true;
                                                }
                                            }
                                        }
                                    }
                                    if poll.pending {
                                        pending = true;
                                        let hint = if poll.next_ms.is_finite() {
                                            poll.next_ms.clamp(1., 16.)
                                        } else {
                                            16.
                                        };
                                        delay = delay.min(hint);
                                    }
                                }
                                if windows.borrow().is_empty() {
                                    let _ = app.update(|cx| cx.quit());
                                    break;
                                }
                                if !pending && !updates {
                                    break;
                                }
                                app.background_executor()
                                    .timer(Duration::from_secs_f64(delay / 1000.))
                                    .await;
                            }
                            running.set(false);
                        })
                        .detach();
                })
            };
            *wake_slot.borrow_mut() = Some(Rc::downgrade(&wake));
            let closed_wake = wake.clone();
            cx.on_window_closed(move |_| closed_wake()).detach();
            for (spec, nodes) in prepared {
                match desktop_open(
                    spec,
                    nodes,
                    events.clone(),
                    failure.clone(),
                    wake.clone(),
                    cx,
                ) {
                    Ok((id, handle)) => {
                        allocated.borrow_mut().insert(id);
                        windows.borrow_mut().insert(id, handle);
                    }
                    Err(error) => {
                        failure.borrow_mut().get_or_insert(error);
                    }
                }
            }
            if windows.borrow().is_empty() {
                cx.quit();
            } else {
                wake();
                cx.activate(true);
            }
        });
    let outcome = result.borrow_mut().take();
    match outcome {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn desktop_open(
    spec: DesktopWindow,
    nodes: Vec<DesktopNode>,
    events: Rc<RefCell<Vec<DesktopEvent>>>,
    failure: Rc<RefCell<Option<String>>>,
    wake: Rc<dyn Fn()>,
    cx: &mut App,
) -> Result<(i64, gpui::WindowHandle<DesktopRoot>), String> {
    validate_window(&spec)?;
    let bounds = Bounds::centered(
        None,
        size(px(spec.width as f32), px(spec.height as f32)),
        cx,
    );
    let id = spec.id;
    let handle = cx
        .open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(spec.title.into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                is_resizable: spec.resizable,
                is_minimizable: spec.minimizable,
                ..Default::default()
            },
            move |window, cx| {
                let root = cx.new(|cx| DesktopRoot {
                    id,
                    nodes,
                    published_revision: 1,
                    acknowledged_edits: HashMap::new(),
                    edit_sequence: Rc::new(Cell::new(0)),
                    events,
                    failure,
                    wake,
                    inputs: Default::default(),
                    focus: Default::default(),
                    focus_order: Vec::new(),
                    splits: Default::default(),
                    pending_focus: Vec::new(),
                    root_focus: cx.focus_handle(),
                    modal: None,
                    modal_previous_focus: None,
                });
                root.read(cx).root_focus.focus(window);
                root
            },
        )
        .map_err(|error| error.to_string())?;
    Ok((id, handle))
}

/// Deterministic tree/event bridge check. This never creates or simulates a
/// native window; real keyboard, pointer and IME checks use the desktop loop.
pub fn desktop_probe(
    window: i64,
    render: impl Fn(i64) -> Vec<DesktopNode> + 'static,
    dispatch: impl Fn(Vec<DesktopEvent>) + 'static,
    events: Vec<DesktopEvent>,
) -> Result<Vec<DesktopNode>, String> {
    if window <= 0 {
        return Err("desktop probe needs a positive window id".into());
    }
    if events.iter().any(|event| event.window != window) {
        return Err("desktop probe event belongs to another window".into());
    }
    let render: Rc<dyn Fn(i64) -> Vec<DesktopNode>> = Rc::new(render);
    let before = render_desktop_tree(window, &render)?;
    if events.is_empty() {
        return Ok(before);
    }
    catch_unwind(AssertUnwindSafe(|| dispatch(events))).map_err(panic_message)?;
    render_desktop_tree(window, &render)
}

#[cfg(test)]
mod desktop_tests {
    use super::*;

    fn node(kind: &str, key: &str, children: Vec<DesktopNode>) -> DesktopNode {
        DesktopNode {
            kind: kind.into(),
            key: key.into(),
            text: String::new(),
            placeholder: String::new(),
            style: Style {
                gap: 0.,
                padding: 0.,
                background: -1,
                color: -1,
                radius: 0.,
                font_size: 14.,
                width: -1.,
                height: -1.,
                grow: 0.,
                shrink: 1.,
            },
            disabled: false,
            read_only: false,
            checked: false,
            multiline: false,
            submit_on_enter: false,
            ratio: 0.5,
            vertical: false,
            source: String::new(),
            language: String::new(),
            action: String::new(),
            tooltip: String::new(),
            accepts_keys: false,
            runs: Vec::new(),
            primitives: Vec::new(),
            children,
        }
    }
    fn window(id: i64) -> DesktopWindow {
        DesktopWindow {
            id,
            title: "test".into(),
            width: 400.,
            height: 300.,
            resizable: true,
            minimizable: true,
        }
    }

    #[test]
    fn fragments_flatten_recursively_and_preserve_owner_paths() {
        let tree = vec![node(
            "fragment",
            "root",
            vec![node(
                "fragment",
                "root/component",
                vec![node(
                    "fragment",
                    "root/component/nested",
                    vec![node("input", "root/component/nested/query", vec![])],
                )],
            )],
        )];
        validate_tree(&tree).unwrap();
        let mut seen = HashSet::new();
        let flat = flatten_fragments(tree.clone(), &mut seen);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].key, "root/component/nested/query");
        assert_eq!(seen.len(), 4);
        assert_eq!(tree[0].children[0].kind, "fragment");
        assert_eq!(event_root_key(&tree), Some("root/component/nested/query"));
    }
    #[test]
    fn fragment_expansion_does_not_flatten_real_container_children() {
        let tree = vec![node(
            "fragment",
            "root",
            vec![node(
                "column",
                "root/main",
                vec![node("input", "root/main/query", vec![])],
            )],
        )];
        let flat = flatten_fragments(tree, &mut HashSet::new());
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].children.len(), 1);
    }
    #[test]
    fn split_counts_expanded_fragment_panes() {
        let tree = vec![node(
            "split",
            "root/split",
            vec![node(
                "fragment",
                "root/split/owner",
                vec![
                    node("column", "root/split/owner/left", vec![]),
                    node("column", "root/split/owner/right", vec![]),
                ],
            )],
        )];
        assert!(validate_tree(&tree).is_ok());
        let invalid = vec![node(
            "split",
            "root/split",
            vec![node("fragment", "root/split/empty", vec![])],
        )];
        assert!(
            validate_tree(&invalid)
                .unwrap_err()
                .contains("exactly two panes")
        );
    }
    #[test]
    fn duplicate_owner_keys_fail_even_when_fragments_have_no_element() {
        let tree = vec![
            node(
                "fragment",
                "root/owner",
                vec![node("input", "root/owner/input", vec![])],
            ),
            node("fragment", "root/owner", vec![]),
        ];
        assert!(validate_tree(&tree).unwrap_err().contains("duplicate"));
    }
    #[test]
    fn focus_accepts_unique_authored_key_and_exact_path() {
        let keys = ["root/sidebar/search", "root/chat/composer"];
        assert_eq!(
            resolve_key("composer", keys.into_iter()).unwrap(),
            "root/chat/composer"
        );
        assert_eq!(
            resolve_key("root/sidebar/search", keys.into_iter()).unwrap(),
            "root/sidebar/search"
        );
        assert!(
            resolve_key("query", keys.into_iter())
                .unwrap_err()
                .contains("not mounted")
        );
    }
    #[test]
    fn focus_preserves_authored_path_and_reserved_character_identity() {
        let keys = ["root/a%2Fb", "root/a/b", "root/%252F", "root/%7E0", "root/~0"];
        assert_eq!(resolve_key("a/b", keys.into_iter()).unwrap(), "root/a%2Fb");
        assert_eq!(resolve_key("%2F", keys.into_iter()).unwrap(), "root/%252F");
        assert_eq!(resolve_key("~0", keys.into_iter()).unwrap(), "root/%7E0");
        assert_eq!(resolve_key("root/a/b", keys.into_iter()).unwrap(), "root/a/b");
    }
    #[test]
    fn focus_reports_ambiguous_local_keys_and_avoids_suffix_substrings() {
        let keys = ["root/a/query", "root/b/query", "root/otherquery"];
        assert!(
            resolve_key("query", keys.into_iter())
                .unwrap_err()
                .contains("ambiguous")
        );
        assert_eq!(
            resolve_key("root/a/query", keys.into_iter()).unwrap(),
            "root/a/query"
        );
        assert_eq!(
            resolve_key("otherquery", keys.into_iter()).unwrap(),
            "root/otherquery"
        );
    }
    #[test]
    fn modal_owner_paths_require_a_component_boundary() {
        assert!(belongs_to("root/dialog/input", "root/dialog"));
        assert!(belongs_to("root/dialog", "root/dialog"));
        assert!(!belongs_to("root/dialog_other/input", "root/dialog"));
    }
    #[test]
    fn publication_acknowledgements_snapshot_only_the_exact_processed_batch() {
        let mut first = desktop_event(1, "root/input".into(), "change");
        first.text = "a".into();
        first.number = 1.;
        let mut later = desktop_event(1, "root/input".into(), "change");
        later.text = "ab".into();
        later.number = 2.;
        let mut processed = EditAcknowledgements::new();
        acknowledge_input_batch(&mut processed, &[first]);
        let publication = publication_acknowledgements(&processed, 1);
        // The second event is queued after dispatch and must not be inferred
        // from the live entity's newest text or token while publishing.
        assert_eq!(publication.get("root/input"), Some(&1));
        acknowledge_input_batch(&mut processed, &[later]);
        assert_eq!(publication.get("root/input"), Some(&1));
        assert_eq!(
            publication_acknowledgements(&processed, 1).get("root/input"),
            Some(&2)
        );
        assert!(publication_acknowledgements(&processed, 2).is_empty());
    }
    #[test]
    fn editing_acknowledgements_ignore_other_numeric_payloads_and_never_go_backwards() {
        let mut processed = EditAcknowledgements::new();
        let mut events = vec![];
        for (kind, number) in [
            ("change", 3.),
            ("change", 2.),
            ("resize", 7.),
            ("change", 0.),
            ("change", 1.5),
            ("change", f64::NAN),
        ] {
            let mut event = desktop_event(1, "root/input".into(), kind);
            event.number = number;
            events.push(event);
        }
        let mut other = desktop_event(2, "root/input".into(), "change");
        other.number = 4.;
        events.push(other);
        acknowledge_input_batch(&mut processed, &events);
        assert_eq!(
            publication_acknowledgements(&processed, 1).get("root/input"),
            Some(&3)
        );
        assert_eq!(
            publication_acknowledgements(&processed, 2).get("root/input"),
            Some(&4)
        );
    }
    #[test]
    fn shortcuts_route_to_the_nearest_focused_owner_through_fragments() {
        let mut editor = node(
            "column",
            "root/editor",
            vec![node(
                "fragment",
                "root/editor/component",
                vec![node("input", "root/editor/component/input", vec![])],
            )],
        );
        editor.accepts_keys = true;
        let mut root = node("column", "root", vec![editor]);
        root.accepts_keys = true;
        let tree = vec![root];
        assert_eq!(
            key_handler(&tree, Some("root/editor/component/input"), None),
            Some("root/editor")
        );
        assert_eq!(
            key_handler(&tree, Some("root/sidebar/button"), None),
            Some("root")
        );
        assert_eq!(key_handler(&tree, None, None), Some("root"));
    }
    #[test]
    fn modal_shortcuts_cannot_escape_to_background_handlers() {
        let mut dialog = node(
            "modal",
            "root/dialog",
            vec![node("input", "root/dialog/input", vec![])],
        );
        let mut root = node("column", "root", vec![dialog.clone()]);
        root.accepts_keys = true;
        let mut tree = vec![root];
        assert_eq!(
            key_handler(&tree, Some("root/dialog/input"), Some("root/dialog")),
            None
        );
        assert_eq!(
            key_handler(&tree, Some("root/background/input"), Some("root/dialog")),
            None
        );
        assert_eq!(key_handler(&tree, None, Some("root/dialog")), None);
        dialog.accepts_keys = true;
        tree[0].children[0] = dialog;
        assert_eq!(
            key_handler(&tree, Some("root/dialog/input"), Some("root/dialog")),
            Some("root/dialog")
        );
        assert_eq!(
            key_handler(&tree, None, Some("root/dialog")),
            Some("root/dialog")
        );
        assert_eq!(
            key_handler(&tree, Some("root/dialog_other/input"), Some("root/dialog")),
            None
        );
    }
    #[test]
    fn nested_modal_only_routes_handlers_inside_the_top_modal() {
        let mut inner = node(
            "modal",
            "root/dialog/nested",
            vec![node("input", "root/dialog/nested/input", vec![])],
        );
        inner.accepts_keys = true;
        let mut outer = node("modal", "root/dialog", vec![inner]);
        outer.accepts_keys = true;
        let tree = vec![outer];
        let modal = last_modal(&tree).unwrap();
        assert_eq!(
            key_handler(&tree, Some("root/dialog/nested/input"), Some(&modal)),
            Some("root/dialog/nested")
        );
        assert_eq!(
            key_handler(&tree, Some("root/dialog/input"), Some(&modal)),
            None
        );
    }
    #[test]
    fn disabled_shortcut_owner_falls_back_to_an_enabled_ancestor() {
        let mut child = node("column", "root/panel", vec![]);
        child.accepts_keys = true;
        child.disabled = true;
        let mut root = node("column", "root", vec![child]);
        root.accepts_keys = true;
        assert_eq!(
            key_handler(&[root], Some("root/panel/input"), None),
            Some("root")
        );
    }
    #[test]
    fn pointer_and_keyboard_activation_share_toggle_and_action_records() {
        for kind in ["checkbox", "switch"] {
            let record = activation(2, "root/control".into(), kind, false, "unused");
            assert_eq!(record.kind, "change");
            assert!(record.checked);
            assert!(record.text.is_empty());
            assert!(!activation(2, "root/control".into(), kind, true, "").checked);
        }
        let record = activation(2, "root/send".into(), "button", false, "send");
        assert_eq!(record.kind, "action");
        assert_eq!(record.text, "send");
        assert_eq!(
            activation(2, "root/send".into(), "button", false, "").kind,
            "click"
        );
    }

    // These use GPUI's real dispatch tree and its KeyUp-generated keyboard
    // ClickEvent, without creating an operating-system window. The native GUI
    // acceptance fixture separately exercises physical mouse/keyboard input.
    #[cfg(feature = "test-support")]
    mod native_keyboard {
        use super::*;
        use gpui::{Keystroke, TestAppContext, VisualTestContext, WindowHandle};

        fn open(
            nodes: Vec<DesktopNode>,
            events: Rc<RefCell<Vec<DesktopEvent>>>,
            cx: &mut TestAppContext,
        ) -> WindowHandle<DesktopRoot> {
            let handle = cx.add_window(move |_, cx| DesktopRoot {
                id: 1,
                nodes,
                published_revision: 1,
                acknowledged_edits: HashMap::new(),
                edit_sequence: Rc::new(Cell::new(0)),
                events,
                failure: Rc::new(RefCell::new(None)),
                wake: Rc::new(|| {}),
                inputs: HashMap::new(),
                focus: HashMap::new(),
                focus_order: vec![],
                splits: HashMap::new(),
                pending_focus: vec![],
                root_focus: cx.focus_handle(),
                modal: None,
                modal_previous_focus: None,
            });
            paint(handle, cx);
            handle
        }

        fn paint(handle: WindowHandle<DesktopRoot>, cx: &mut TestAppContext) {
            cx.update_window(*handle, |_, window, cx| window.draw(cx).clear())
                .unwrap();
        }

        fn focus(handle: WindowHandle<DesktopRoot>, key: &str, cx: &mut TestAppContext) {
            handle
                .update(cx, |root, window, cx| {
                    root.focus_handle(key, cx).unwrap().focus(window);
                })
                .unwrap();
            paint(handle, cx);
        }

        fn down(
            handle: WindowHandle<DesktopRoot>,
            key: &str,
            held: bool,
            cx: &mut TestAppContext,
        ) {
            VisualTestContext::from_window(*handle, cx).simulate_event(gpui::KeyDownEvent {
                keystroke: Keystroke::parse(key).unwrap(),
                is_held: held,
            });
        }

        fn up(handle: WindowHandle<DesktopRoot>, key: &str, cx: &mut TestAppContext) {
            VisualTestContext::from_window(*handle, cx).simulate_event(gpui::KeyUpEvent {
                keystroke: Keystroke::parse(key).unwrap(),
            });
        }

        fn control(kind: &str, key: &str) -> DesktopNode {
            let mut control = node(kind, key, vec![]);
            control.style.width = 240.;
            control.style.height = 44.;
            control
        }

        fn column_fixture(children: Vec<DesktopNode>) -> DesktopNode {
            let mut root = node("column", "root", children);
            root.style.width = 600.;
            root.style.height = 480.;
            root.style.padding = 20.;
            root.style.gap = 12.;
            root
        }

        fn bounds(
            handle: WindowHandle<DesktopRoot>,
            key: &'static str,
            cx: &mut TestAppContext,
        ) -> Bounds<gpui::Pixels> {
            VisualTestContext::from_window(*handle, cx)
                .debug_bounds(key)
                .unwrap_or_else(|| panic!("missing painted bounds for {key}"))
        }

        fn assert_pixels(actual: gpui::Pixels, expected: f32) {
            assert!((actual - px(expected)).abs() < px(0.01), "{actual:?} != {expected}");
        }

        #[gpui::test]
        fn split_keeps_explicit_height_and_auto_split_fills_remaining_column(cx: &mut TestAppContext) {
            for (height, expected_height, next_y) in [(220., 220., 292.), (-1., 344., 416.)] {
                let mut label = node("text", "root/label", vec![]);
                label.style.height = 28.;
                let mut split = node("split", "root/panes", vec![
                    node("column", "root/panes/first", vec![]),
                    node("column", "root/panes/second", vec![]),
                ]);
                split.style.width = 560.;
                split.style.height = height;
                let mut next = control("button", "root/next");
                next.style.width = 560.;
                let events = Rc::new(RefCell::new(vec![]));
                let handle = open(vec![column_fixture(vec![label, split, next])], events.clone(), cx);
                let split = bounds(handle, "root/panes", cx);
                assert_pixels(split.origin.x, 20.);
                assert_pixels(split.origin.y, 60.);
                assert_pixels(split.size.width, 560.);
                assert_pixels(split.size.height, expected_height);
                let next = bounds(handle, "root/next", cx);
                assert_pixels(next.origin.y, next_y);
                assert_pixels(next.size.height, 44.);
                VisualTestContext::from_window(*handle, cx).simulate_click(
                    gpui::point(px(70.), px(next_y + 22.)),
                    gpui::Modifiers::default(),
                );
                let batch = std::mem::take(&mut *events.borrow_mut());
                assert_eq!(batch.len(), 1, "the following button must remain reachable");
                assert_eq!(batch[0].node, "root/next");
                assert_eq!(batch[0].kind, "click");
            }
        }

        #[gpui::test]
        fn split_respects_explicit_width_in_a_row_and_can_auto_fill_width(cx: &mut TestAppContext) {
            for (width, expected_width) in [(220., 220.), (-1., 404.)] {
                let mut before = control("button", "root/before");
                before.style.width = 64.;
                let mut split = node("split", "root/panes", vec![
                    node("column", "root/panes/first", vec![]),
                    node("column", "root/panes/second", vec![]),
                ]);
                split.style.width = width;
                let mut next = control("button", "root/next");
                next.style.width = 44.;
                let mut root = node("row", "root", vec![before, split, next]);
                root.style.width = 560.;
                root.style.height = 300.;
                root.style.padding = 12.;
                root.style.gap = 12.;
                let handle = open(vec![root], Rc::new(RefCell::new(vec![])), cx);
                let split = bounds(handle, "root/panes", cx);
                assert_pixels(split.size.width, expected_width);
                assert_pixels(split.size.height, 276.);
                let next = bounds(handle, "root/next", cx);
                assert_pixels(next.origin.x, 12. + 64. + 12. + expected_width + 12.);
            }
        }

        #[gpui::test]
        fn scroll_keeps_explicit_height_and_auto_scroll_fills_remaining_column(cx: &mut TestAppContext) {
            for (height, expected_height, next_y) in [(360., 360., 392.), (-1., 384., 416.)] {
                let mut content = node("column", "root/messages/content", vec![]);
                content.style.height = 1000.;
                let mut scroll = node("scroll", "root/messages", vec![content]);
                scroll.style.width = 560.;
                scroll.style.height = height;
                let next = control("button", "root/next");
                let handle = open(
                    vec![column_fixture(vec![scroll, next])],
                    Rc::new(RefCell::new(vec![])),
                    cx,
                );
                assert_pixels(bounds(handle, "root/messages", cx).size.height, expected_height);
                assert_pixels(bounds(handle, "root/next", cx).origin.y, next_y);
            }
        }

        #[gpui::test]
        fn scroll_cards_keep_natural_text_widths_and_contain_markdown_content(cx: &mut TestAppContext) {
            fn leaf(kind: &str, key: &str, text: &str) -> DesktopNode {
                let mut item = node(kind, key, vec![]);
                item.text = text.into();
                item.style.gap = 8.;
                item
            }
            let mut cards = vec![];
            for (index, body) in [
                "Review the workspace and its Unicode files.",
                "# Workspace review\n\nThe workspace is ready.\n\n```zolo\nlet greeting = \"ação🦀日本\"\n```\n",
                "Please inspect the workspace and show the result.",
                "## Implementation\n\nThis assistant message has several paragraphs.\n\nMarkdown must fit inside its own card without painting over the next message.\n",
            ].into_iter().enumerate() {
                let key = format!("root/messages/card{index}");
                let mut role = leaf("badge", &format!("{key}/header/role"), if index % 2 == 0 { "user" } else { "assistant" });
                role.style.padding = 4.;
                role.style.font_size = 12.;
                role.style.shrink = 0.;
                let mut status = leaf("text", &format!("{key}/header/status"), "complete");
                status.style.font_size = 12.;
                status.style.shrink = 0.;
                let mut header = node("row", &format!("{key}/header"), vec![
                    role, leaf("spacer", &format!("{key}/header/spacer"), ""), status,
                ]);
                header.style.gap = 8.;
                let mut card = node("column", &key, vec![header, leaf("markdown", &format!("{key}/body"), body)]);
                card.style.padding = 12.;
                card.style.gap = 8.;
                cards.push(card);
            }
            let mut title = leaf("text", "root/messages/tool/toggle/caption/title", "Inspect workspace");
            title.style.grow = 1.;
            let mut status = leaf("badge", "root/messages/tool/toggle/caption/status", "complete");
            status.style.padding = 4.;
            status.style.font_size = 12.;
            status.style.shrink = 0.;
            let mut caption = node("row", "root/messages/tool/toggle/caption", vec![
                title,
                status,
            ]);
            caption.style.gap = 8.;
            let mut toggle = control("button", "root/messages/tool/toggle");
            toggle.style.width = -1.;
            toggle.style.height = -1.;
            toggle.style.padding = 8.;
            toggle.children = vec![caption];
            let mut tool = node("column", "root/messages/tool", vec![
                toggle,
                leaf("code", "root/messages/tool/output", "Workspace ready\nREADME.md\nmain.zolo\nUnicode preserved\nTool finished"),
            ]);
            tool.style.padding = 8.;
            tool.style.gap = 8.;
            cards.push(tool);
            let mut scroll = node("scroll", "root/messages", cards);
            scroll.style.height = 360.;
            scroll.style.shrink = 0.;
            scroll.style.gap = 8.;
            let mut root = node("column", "root", vec![scroll]);
            root.style.width = 640.;
            root.style.height = 600.;
            root.style.padding = 16.;
            let handle = open(vec![root], Rc::new(RefCell::new(vec![])), cx);
            let status = bounds(handle, "root/messages/card0/header/status", cx);
            assert!(status.size.width > px(32.), "message status collapsed into a vertical strip");
            assert!(status.size.height < px(32.), "message status wrapped character by character");
            let title = bounds(handle, "root/messages/tool/toggle/caption/title", cx);
            assert!(title.size.width > px(80.), "tool title collapsed into a vertical strip");
            let card = bounds(handle, "root/messages/card1", cx);
            let body = bounds(handle, "root/messages/card1/body", cx);
            assert!(body.bottom() <= card.bottom() - px(11.), "Markdown paints outside its card");
            let following = bounds(handle, "root/messages/card2", cx);
            assert!(following.top() >= card.bottom(), "following card overlaps the previous card");
            let tool = bounds(handle, "root/messages/tool", cx);
            let output = bounds(handle, "root/messages/tool/output", cx);
            assert!(output.bottom() <= tool.bottom() - px(7.), "tool output paints outside its card");
            assert!(tool.bottom() > bounds(handle, "root/messages", cx).bottom(), "content should overflow the viewport rather than compress into it");
        }

        #[gpui::test]
        fn application_wrapper_preserves_explicit_panel_growth_and_intrinsic_scroll_content(cx: &mut TestAppContext) {
            let mut heading = node("text", "application/panel/header", vec![]);
            heading.style.height = 32.;
            let mut message = node("markdown", "application/panel/split/chat/messages/body", vec![]);
            message.text = (0..20).map(|index| format!("Paragraph {index}: the chat retains its content height.\n\n")).collect();
            let mut card = node("column", "application/panel/split/chat/messages/card", vec![message]);
            card.style.padding = 12.;
            let mut messages = node("scroll", "application/panel/split/chat/messages", vec![card]);
            messages.style.height = 360.;
            messages.style.shrink = 0.;
            let mut composer = control("input", "application/panel/split/chat/composer");
            composer.style.width = -1.;
            composer.style.height = 60.;
            let mut chat = node("column", "application/panel/split/chat", vec![messages, composer]);
            chat.style.gap = 12.;
            let workspace = node("column", "application/panel/split/workspace", vec![]);
            let split = node("split", "application/panel/split", vec![workspace, chat]);
            let mut panel = node("column", "application/panel", vec![heading, split]);
            panel.style.grow = 1.;
            panel.style.gap = 12.;
            let mut application = node("column", "application", vec![panel]);
            application.style.width = 900.;
            application.style.height = 600.;
            application.style.padding = 20.;
            let handle = open(vec![application], Rc::new(RefCell::new(vec![])), cx);
            assert_pixels(bounds(handle, "application/panel", cx).size.height, 560.);
            assert_pixels(bounds(handle, "application/panel/split", cx).size.height, 516.);
            assert_pixels(bounds(handle, "application/panel/split/chat", cx).size.height, 516.);
            assert_pixels(bounds(handle, "application/panel/split/chat/messages", cx).size.height, 360.);
            let body = bounds(handle, "application/panel/split/chat/messages/body", cx);
            let card = bounds(handle, "application/panel/split/chat/messages/card", cx);
            assert!(body.bottom() <= card.bottom() - px(11.));
            assert!(card.size.height > px(360.), "long chat content must remain scrollable");
            assert_pixels(bounds(handle, "application/panel/split/chat/composer", cx).origin.y, 436.);
        }

        #[gpui::test]
        fn explicit_flex_factors_control_space_and_fixed_scroll_never_shrinks(cx: &mut TestAppContext) {
            let mut first = node("column", "root/first", vec![]);
            first.style.grow = 1.;
            let mut second = node("column", "root/second", vec![]);
            second.style.grow = 2.;
            let mut root = node("column", "root", vec![first, second]);
            root.style.width = 600.;
            root.style.height = 300.;
            let handle = open(vec![root], Rc::new(RefCell::new(vec![])), cx);
            assert_pixels(bounds(handle, "root/first", cx).size.height, 100.);
            assert_pixels(bounds(handle, "root/second", cx).size.height, 200.);

            let mut scroll = node("scroll", "root/scroll", vec![]);
            scroll.style.height = 360.;
            scroll.style.shrink = 0.;
            let mut sibling = node("column", "root/sibling", vec![]);
            sibling.style.height = 100.;
            let mut root = node("column", "root", vec![scroll, sibling]);
            root.style.width = 600.;
            root.style.height = 300.;
            let handle = open(vec![root], Rc::new(RefCell::new(vec![])), cx);
            assert_pixels(bounds(handle, "root/scroll", cx).size.height, 360.);
            assert_pixels(bounds(handle, "root/sibling", cx).origin.y, 360.);
        }

        #[gpui::test]
        fn one_activation_per_focused_control_key_release(cx: &mut TestAppContext) {
            for kind in ["button", "checkbox", "switch", "tab", "select_item"] {
                let mut target = control(kind, "root/panel/control");
                if kind == "button" {
                    target.action = "send".into();
                }
                let mut panel = node("column", "root/panel", vec![target]);
                panel.accepts_keys = true;
                let mut root = node("column", "root", vec![panel]);
                root.accepts_keys = true;
                let events = Rc::new(RefCell::new(vec![]));
                let handle = open(vec![root], events.clone(), cx);
                focus(handle, "root/panel/control", cx);

                for key in ["space", "enter"] {
                    down(handle, key, false, cx);
                    // Auto-repeat must not create extra activations, either.
                    down(handle, key, true, cx);
                    assert!(events.borrow().is_empty(), "{kind}: key-down activated or leaked");
                    up(handle, key, cx);
                    let batch = std::mem::take(&mut *events.borrow_mut());
                    assert_eq!(batch.len(), 1, "{kind}: {key} must activate exactly once");
                    assert_eq!(batch[0].node, "root/panel/control");
                    match kind {
                        "checkbox" | "switch" => {
                            assert_eq!(batch[0].kind, "change");
                            assert!(batch[0].checked);
                        }
                        "button" => {
                            assert_eq!(batch[0].kind, "action");
                            assert_eq!(batch[0].text, "send");
                        }
                        _ => assert_eq!(batch[0].kind, "click"),
                    }
                }
                down(handle, "ctrl-enter", false, cx);
                up(handle, "ctrl-enter", cx);
                let batch = std::mem::take(&mut *events.borrow_mut());
                assert_eq!(batch.len(), 1, "modified Enter must remain one ancestor shortcut");
                assert_eq!(batch[0].kind, "key");
                assert_eq!(batch[0].node, "root/panel");
                assert_eq!(batch[0].key, "ctrl-enter");
            }
        }

        #[gpui::test]
        fn unfocused_parent_control_cannot_activate_for_focused_child(cx: &mut TestAppContext) {
            let child = control("button", "root/parent/child");
            let mut parent = control("button", "root/parent");
            parent.children = vec![child];
            let mut root = node("column", "root", vec![parent]);
            root.accepts_keys = true;
            let events = Rc::new(RefCell::new(vec![]));
            let handle = open(vec![root], events.clone(), cx);
            focus(handle, "root/parent/child", cx);
            down(handle, "enter", false, cx);
            assert!(events.borrow().is_empty());
            up(handle, "enter", cx);
            let batch = std::mem::take(&mut *events.borrow_mut());
            assert_eq!(batch.len(), 1);
            assert_eq!(batch[0].node, "root/parent/child");
            assert_eq!(batch[0].kind, "click");
        }

        #[gpui::test]
        fn modal_control_activation_and_shortcuts_stay_inside_the_modal(cx: &mut TestAppContext) {
            let target = control("button", "root/dialog/panel/send");
            let mut panel = node("column", "root/dialog/panel", vec![target]);
            panel.accepts_keys = true;
            let dialog = node("modal", "root/dialog", vec![panel]);
            let mut root = node("column", "root", vec![dialog]);
            root.accepts_keys = true;
            let events = Rc::new(RefCell::new(vec![]));
            let handle = open(vec![root], events.clone(), cx);
            focus(handle, "root/dialog/panel/send", cx);
            down(handle, "enter", false, cx);
            assert!(events.borrow().is_empty());
            up(handle, "enter", cx);
            let batch = std::mem::take(&mut *events.borrow_mut());
            assert_eq!(batch.len(), 1);
            assert_eq!(batch[0].node, "root/dialog/panel/send");
            assert_eq!(batch[0].kind, "click");

            down(handle, "ctrl-enter", false, cx);
            up(handle, "ctrl-enter", cx);
            let batch = std::mem::take(&mut *events.borrow_mut());
            assert_eq!(batch.len(), 1);
            assert_eq!(batch[0].node, "root/dialog/panel");
            assert_eq!(batch[0].kind, "key");

            handle
                .update(cx, |root, _, cx| {
                    root.nodes[0].children[0].children[0].accepts_keys = false;
                    cx.notify();
                })
                .unwrap();
            paint(handle, cx);
            down(handle, "ctrl-enter", false, cx);
            up(handle, "ctrl-enter", cx);
            assert!(events.borrow().is_empty(), "modal leaked to the background handler");

            down(handle, "escape", false, cx);
            up(handle, "escape", cx);
            let batch = std::mem::take(&mut *events.borrow_mut());
            assert_eq!(batch.len(), 1);
            assert_eq!(batch[0].node, "root/dialog");
            assert_eq!(batch[0].kind, "dismiss");
        }
    }

    #[test]
    fn duplicate_dynamic_window_batch_rejects_existing_and_internal_duplicates() {
        assert!(
            validate_window_batch(&[window(2), window(2)], &HashSet::new())
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(
            validate_window_batch(&[window(2)], &HashSet::from([2]))
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(validate_window_batch(&[window(2), window(3)], &HashSet::from([1])).is_ok());
    }
    #[test]
    fn duplicate_initial_windows_fail_before_render_or_native_start() {
        let count = Rc::new(Cell::new(0));
        let render_count = count.clone();
        let result = desktop(
            vec![window(1), window(1)],
            false,
            move |_| {
                render_count.set(render_count.get() + 1);
                vec![]
            },
            |_| {},
            || DesktopTurn {
                windows: vec![],
                commands: vec![],
                polls: vec![],
            },
            |_| false,
        );
        assert!(result.unwrap_err().contains("duplicate"));
        assert_eq!(count.get(), 0);
    }
    #[test]
    fn dimensions_must_be_finite_after_conversion_to_native_pixels() {
        let mut spec = window(1);
        spec.width = f64::MAX;
        assert!(validate_window(&spec).is_err());
        spec.width = f64::NAN;
        assert!(validate_window(&spec).is_err());
        let mut tree = node("column", "root", vec![]);
        tree.style.height = f64::INFINITY;
        assert!(validate_tree(&[tree]).unwrap_err().contains("non-finite"));
    }
    #[test]
    fn flex_factors_must_be_nonnegative_and_finite_native_values() {
        for value in [-1., f64::NAN, f64::INFINITY, f64::MAX] {
            let mut tree = node("column", "root", vec![]);
            tree.style.grow = value;
            assert!(validate_tree(&[tree.clone()]).is_err(), "accepted grow {value}");
            tree.style.grow = 0.;
            tree.style.shrink = value;
            assert!(validate_tree(&[tree]).is_err(), "accepted shrink {value}");
        }
        let mut tree = node("column", "root", vec![]);
        tree.style.grow = 2.5;
        tree.style.shrink = 0.;
        validate_tree(&[tree]).unwrap();
    }
    #[test]
    fn split_local_drag_survives_stale_echo_until_external_value_changes() {
        let mut split = SplitState {
            ratio: 0.5,
            external_ratio: 0.5,
            vertical: false,
            bounds: Bounds::new(gpui::point(px(10.), px(20.)), size(px(406.), px(300.))),
            dragging: true,
        };
        assert!(split.drag(gpui::point(px(113.), px(20.))));
        assert!((split.ratio - 0.25).abs() < 0.001);
        split.sync(0.5, false);
        assert!((split.ratio - 0.25).abs() < 0.001);
        split.sync(0.25, false);
        assert!((split.ratio - 0.25).abs() < 0.001);
        split.sync(0.7, true);
        assert_eq!(split.ratio, 0.7);
        assert!(!split.dragging);
    }
    #[test]
    fn split_drag_clamps_both_axes_to_usable_panes() {
        let mut split = SplitState {
            ratio: 0.5,
            external_ratio: 0.5,
            vertical: true,
            bounds: Bounds::new(gpui::point(px(0.), px(0.)), size(px(200.), px(406.))),
            dragging: true,
        };
        split.drag(gpui::point(px(0.), px(-100.)));
        assert_eq!(split.ratio, 0.1);
        split.drag(gpui::point(px(0.), px(1000.)));
        assert_eq!(split.ratio, 0.9);
    }
    #[test]
    fn resize_coalescing_preserves_other_events_and_wakes_after_queue_borrow() {
        let queue = Rc::new(RefCell::new(Vec::new()));
        let wake_queue = queue.clone();
        let wake: Rc<dyn Fn()> = Rc::new(move || {
            assert!(!wake_queue.borrow().is_empty());
        });
        let mut first = desktop_event(1, "root/split".into(), "resize");
        first.number = 0.2;
        queue_event(&queue, first, &wake);
        let mut second = desktop_event(1, "root/split".into(), "resize");
        second.number = 0.3;
        queue_event(&queue, second, &wake);
        queue_event(&queue, desktop_event(1, "root/send".into(), "click"), &wake);
        assert_eq!(queue.borrow().len(), 2);
        assert_eq!(queue.borrow()[0].number, 0.3);
    }
    #[test]
    fn commonmark_preserves_nested_emphasis_unicode_links_and_fenced_code() {
        let blocks = markdown_blocks(
            "# Title\n\nOlá **bold *italic*** [link](https://example.test/a) and `x`.\n\n```zolo\nlet x = 1\n```\n\n> quoted\n",
        );
        assert_eq!(blocks[0].heading, 1);
        assert!(
            blocks[1]
                .runs
                .iter()
                .any(|run| run.bold && run.italic && run.text == "italic")
        );
        assert!(
            blocks[1]
                .runs
                .iter()
                .any(|run| run.link == "https://example.test/a" && run.text == "link")
        );
        assert!(blocks[1].runs.iter().any(|run| run.code && run.text == "x"));
        let code = blocks.iter().find(|block| block.kind == "code").unwrap();
        assert_eq!(code.language, "zolo");
        assert_eq!(code.runs[0].text, "let x = 1\n");
        assert!(blocks.iter().any(|block| block.kind == "quote"));
    }
    #[test]
    fn streaming_markdown_handles_open_fences_and_raw_html_as_text() {
        let blocks = markdown_blocks("```rust\nlet x = 🦀\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, "code");
        assert_eq!(blocks[0].runs[0].text, "let x = 🦀\n");
        let html = markdown_blocks("<script>alert(1)</script>");
        assert!(
            html.iter()
                .flat_map(|block| &block.runs)
                .any(|run| run.text.contains("<script>"))
        );
    }
    #[test]
    fn desktop_probe_dispatches_typed_events_and_validates_updated_tree() {
        let value = Rc::new(Cell::new(false));
        let render_value = value.clone();
        let dispatch_value = value.clone();
        let event = activation(1, "root/toggle".into(), "checkbox", false, "");
        let tree = desktop_probe(
            1,
            move |_| {
                let mut control = node("checkbox", "root/toggle", vec![]);
                control.checked = render_value.get();
                vec![control]
            },
            move |events| dispatch_value.set(events[0].checked),
            vec![event],
        )
        .unwrap();
        assert!(tree[0].checked);
    }
    #[test]
    fn desktop_probe_does_not_accept_another_windows_event() {
        let result = desktop_probe(
            1,
            |_| vec![],
            |_| panic!("must not dispatch"),
            vec![desktop_event(2, "root".into(), "click")],
        );
        assert!(result.err().unwrap().contains("another window"));
    }
}

// Owned records crossing the language boundary; GPUI objects stay in Rust.
#[derive(Clone)]
pub struct Style {
    pub gap: f64,
    pub padding: f64,
    pub background: i64,
    pub color: i64,
    pub radius: f64,
    pub font_size: f64,
    pub width: f64,
    pub height: f64,
    pub grow: f64,
    pub shrink: f64,
}

pub struct NativeNode {
    pub kind: String,
    pub key: String,
    pub text: String,
    pub style: Style,
    pub event: i64,
    pub children: Vec<NativeNode>,
}

struct Root {
    render: Rc<dyn Fn() -> Vec<NativeNode>>,
    dispatch: Rc<dyn Fn(i64)>,
    failure: Rc<RefCell<Option<String>>>,
    quit: Rc<dyn Fn()>,
    wake: Rc<dyn Fn()>,
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|value| value.to_string()))
        .unwrap_or_else(|| "Zolo UI callback failed".into())
}

fn callback<T>(
    failure: &Rc<RefCell<Option<String>>>,
    quit: &Rc<dyn Fn()>,
    f: impl FnOnce() -> T,
) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(panic) => {
            failure
                .borrow_mut()
                .get_or_insert_with(|| panic_message(panic));
            quit();
            None
        }
    }
}

fn element(node: NativeNode, cx: &mut Context<Root>) -> AnyElement {
    let style = node.style;
    let mut view = div().id(SharedString::from(node.key)).flex();
    if node.kind == "row" {
        view = view.flex_row();
    } else {
        view = view.flex_col();
    }
    view = view
        .gap(px(style.gap as f32))
        .p(px(style.padding as f32))
        .rounded(px(style.radius as f32))
        .text_size(px(style.font_size as f32));
    view.style().flex_grow = Some(style.grow as f32);
    view.style().flex_shrink = Some(style.shrink as f32);
    if style.background >= 0 {
        view = view.bg(rgb(style.background as u32));
    }
    if style.color >= 0 {
        view = view.text_color(rgb(style.color as u32));
    }
    if style.width >= 0.0 {
        view = view.w(px(style.width as f32));
    }
    if style.height >= 0.0 {
        view = view.h(px(style.height as f32));
    }
    if !node.text.is_empty() {
        view = view.child(node.text);
    }
    if node.event > 0 {
        let id = node.event;
        view = view.cursor_pointer().on_click(cx.listener(
            move |root: &mut Root, _, _: &mut Window, cx: &mut Context<'_, Root>| {
                callback(&root.failure, &root.quit, || (root.dispatch)(id));
                (root.wake)();
                Context::notify(cx);
            },
        ));
    }
    for child in node.children {
        view = view.children(elements(child, cx));
    }
    view.into_any_element()
}

fn elements(node: NativeNode, cx: &mut Context<Root>) -> Vec<AnyElement> {
    if node.kind == "fragment" {
        node.children
            .into_iter()
            .flat_map(|child| elements(child, cx))
            .collect()
    } else {
        vec![element(node, cx)]
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .size_full()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5));
        let nodes = callback(&self.failure, &self.quit, || (self.render)()).unwrap_or_default();
        (self.wake)();
        for node in nodes {
            root = root.children(elements(node, cx));
        }
        root
    }
}

/// Owns the native event loop until every window closes. Both callbacks are
/// called only on this invocation's thread, before this function returns.
pub fn run(
    title: String,
    width: f64,
    height: f64,
    render: impl Fn() -> Vec<NativeNode> + 'static,
    dispatch: impl Fn(i64) + 'static,
) -> Result<(), String> {
    application(
        title,
        width,
        height,
        false,
        render,
        dispatch,
        NativePoll::idle,
        || false,
        None,
    )
}

/// Poll the host between callbacks on this application's foreground thread.
/// The invocation owns all three callbacks until the last window closes.
pub fn run_updates(
    title: String,
    width: f64,
    height: f64,
    updates: bool,
    render: impl Fn() -> Vec<NativeNode> + 'static,
    dispatch: impl Fn(i64) + 'static,
    update: impl Fn() -> bool + 'static,
) -> Result<(), String> {
    application(
        title,
        width,
        height,
        updates,
        render,
        dispatch,
        move || NativePoll {
            changed: update(),
            ..NativePoll::idle()
        },
        || false,
        None,
    )
}

/// Scheduling hint produced by Zolo's owner-thread task driver.
pub struct NativePoll {
    pub changed: bool,
    pub pending: bool,
    pub next_ms: f64,
}

impl NativePoll {
    fn idle() -> Self {
        Self {
            changed: false,
            pending: false,
            next_ms: -1.0,
        }
    }
}

/// The platform owns the native loop; Zolo owns tasks and cancellation.
pub fn run_tasks(
    title: String,
    width: f64,
    height: f64,
    updates: bool,
    render: impl Fn() -> Vec<NativeNode> + 'static,
    dispatch: impl Fn(i64) + 'static,
    update: impl Fn() -> NativePoll + 'static,
    close: impl Fn() -> bool + 'static,
) -> Result<(), String> {
    application(
        title, width, height, updates, render, dispatch, update, close, None,
    )
}

fn application(
    title: String,
    width: f64,
    height: f64,
    updates: bool,
    render: impl Fn() -> Vec<NativeNode> + 'static,
    dispatch: impl Fn(i64) + 'static,
    update: impl Fn() -> NativePoll + 'static,
    close: impl Fn() -> bool + 'static,
    smoke_point: Option<(f64, f64)>,
) -> Result<(), String> {
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err("window dimensions must be finite and positive".into());
    }
    let failure = Rc::new(RefCell::new(None));
    let error = failure.clone();
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(width as f32), px(height as f32)), cx);
        let root_failure = error.clone();
        let executor = cx.foreground_executor().clone();
        let app = cx.to_async();
        let running = Rc::new(Cell::new(false));
        let closing = Rc::new(Cell::new(false));
        let driver_window = Rc::new(Cell::new(None::<gpui::WindowHandle<Root>>));
        let update = Rc::new(update);
        let close = Rc::new(close);
        let wake: Rc<dyn Fn()> = {
            let running = running.clone();
            let closing = closing.clone();
            let window = driver_window.clone();
            let failure = error.clone();
            Rc::new(move || {
                if running.replace(true) {
                    return;
                }
                let running = running.clone();
                let closing = closing.clone();
                let window = window.clone();
                let failure = failure.clone();
                let app = app.clone();
                let update = update.clone();
                let close = close.clone();
                executor
                    .spawn(async move {
                        loop {
                            // Enter Zolo outside every App/entity mutable borrow.
                            let poll = catch_unwind(AssertUnwindSafe(|| {
                                if closing.get() {
                                    NativePoll {
                                        changed: false,
                                        pending: close(),
                                        next_ms: 1.0,
                                    }
                                } else {
                                    update()
                                }
                            }));
                            let poll = match poll {
                                Ok(poll) => poll,
                                Err(panic) => {
                                    failure
                                        .borrow_mut()
                                        .get_or_insert_with(|| panic_message(panic));
                                    if closing.replace(true) {
                                        let _ = app.update(|cx| cx.quit());
                                        break;
                                    }
                                    continue;
                                }
                            };
                            if closing.get() && !poll.pending {
                                let _ = app.update(|cx| cx.quit());
                                break;
                            }
                            if poll.changed && !closing.get() {
                                if let Some(window) = window.get() {
                                    let _ = window
                                        .update(&mut app.clone(), |_, _, cx| Context::notify(cx));
                                }
                            }
                            if !poll.pending && !updates {
                                break;
                            }
                            // Until host wakeup tokens exist, retry pending native
                            // completions on a bounded timer. Production idle stops.
                            let delay = if poll.pending {
                                poll.next_ms.clamp(1.0, 16.0)
                            } else {
                                16.0
                            };
                            app.background_executor()
                                .timer(Duration::from_secs_f64(delay / 1000.0))
                                .await;
                        }
                        running.set(false);
                    })
                    .detach();
            })
        };
        let closed_wake = wake.clone();
        let closed_flag = closing.clone();
        cx.on_window_closed(move |cx| {
            if cx.windows().is_empty() {
                closed_flag.set(true);
                closed_wake();
            }
        })
        .detach();
        let quit_wake = wake.clone();
        let quit: Rc<dyn Fn()> = Rc::new(move || {
            closing.set(true);
            quit_wake();
        });
        let root_wake = wake.clone();
        let result = cx.open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|_| Root {
                    render: Rc::new(render),
                    dispatch: Rc::new(dispatch),
                    failure: root_failure,
                    quit,
                    wake: root_wake,
                })
            },
        );
        match result {
            Err(e) => {
                *error.borrow_mut() = Some(e.to_string());
                cx.quit();
            }
            Ok(window) => {
                driver_window.set(Some(window));
                wake();
                if let Some((x, y)) = smoke_point {
                    let smoke_failure = error.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(500))
                            .await;
                        if let Err(error) = click_smoke_window(x, y) {
                            *smoke_failure.borrow_mut() = Some(error);
                        }
                        cx.background_executor()
                            .timer(Duration::from_millis(500))
                            .await;
                        let _ = window.update(cx, |_, window, _| window.remove_window());
                    })
                    .detach();
                }
            }
        }
        cx.activate(true);
    });
    let result = failure.borrow_mut().take();
    match result {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(target_os = "windows")]
fn click_smoke_window(x: f64, y: f64) -> Result<(), String> {
    // GPUI 0.2.2's public dispatch_event returns a private type. Deliver the
    // standard OS messages to this thread's smoke window instead.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> *mut std::ffi::c_void;
        fn GetWindowThreadProcessId(window: *mut std::ffi::c_void, process: *mut u32) -> u32;
        fn GetDpiForWindow(window: *mut std::ffi::c_void) -> u32;
        fn SendMessageW(
            window: *mut std::ffi::c_void,
            message: u32,
            wparam: usize,
            lparam: isize,
        ) -> isize;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    let title: Vec<u16> = "Zolo GPUI smoke test\0".encode_utf16().collect();
    unsafe {
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        if window.is_null()
            || GetWindowThreadProcessId(window, std::ptr::null_mut()) != GetCurrentThreadId()
        {
            return Err("smoke window was not created on the invocation thread".into());
        }
        let scale = GetDpiForWindow(window) as f64 / 96.0;
        let x = (x * scale).round() as i32;
        let y = (y * scale).round() as i32;
        let position = ((y & 0xffff) << 16 | (x & 0xffff)) as isize;
        SendMessageW(window, 0x0200, 0, position);
        SendMessageW(window, 0x0201, 1, position);
        SendMessageW(window, 0x0202, 0, position);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn click_smoke_window(_: f64, _: f64) -> Result<(), String> {
    Err("the real-window smoke driver currently supports Windows".into())
}

/// Opens a real window, sends a GPUI mouse click, then closes it. This is an
/// explicit testing operation; normal applications never inject input.
pub fn smoke(
    render: impl Fn() -> Vec<NativeNode> + 'static,
    dispatch: impl Fn(i64) + 'static,
    x: f64,
    y: f64,
) -> Result<(), String> {
    application(
        "Zolo GPUI smoke test".into(),
        400.0,
        240.0,
        false,
        render,
        dispatch,
        NativePoll::idle,
        || false,
        Some((x, y)),
    )
}

/// The real-window integration driver with the normal async lifecycle.
pub fn smoke_tasks(
    render: impl Fn() -> Vec<NativeNode> + 'static,
    dispatch: impl Fn(i64) + 'static,
    update: impl Fn() -> NativePoll + 'static,
    close: impl Fn() -> bool + 'static,
    x: f64,
    y: f64,
) -> Result<(), String> {
    application(
        "Zolo GPUI smoke test".into(),
        400.0,
        240.0,
        false,
        render,
        dispatch,
        update,
        close,
        Some((x, y)),
    )
}

/// Deterministic bridge probe, using the same records and event dispatcher as
/// the GUI. Useful on CI without a display; it does not create a fake window.
pub fn probe(
    render: impl Fn() -> Vec<NativeNode> + 'static,
    dispatch: impl Fn(i64) + 'static,
    event: i64,
) -> Vec<NativeNode> {
    let before = render();
    if event > 0 {
        dispatch(event);
        render()
    } else {
        before
    }
}
