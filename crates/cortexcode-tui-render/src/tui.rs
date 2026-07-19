//! The TUI differential renderer, ported from `tui.ts`'s `TUI` class.
//!
//! Two behavioral simplifications relative to the TypeScript original are
//! made deliberately (documented at each site below), since they are pure
//! performance optimizations that do not change what gets written to the
//! terminal:
//!
//! 1. No reference-identity flatten memoization ([`crate::Container`] always
//!    re-renders and re-concatenates children; the differential writer
//!    below still only rewrites lines whose *content* changed).
//! 2. No `requestAnimationFrame`-style 16ms render coalescing: every
//!    `request_render()` renders immediately (TypeScript's `expediteRender`
//!    path, used unconditionally).
//!
//! Threading: `cortexcode_tui_terminal::Terminal::start` requires `Send`
//! callbacks because it reads stdin on a background thread, but this
//! module's component tree uses `Rc<RefCell<dyn Component>>` (matching the
//! original's mutable object graph) which is not `Send`. So unlike the
//! TypeScript original — where `terminal.start()`'s callbacks call
//! straight into `TUI.handleInput`/`requestRender` — [`Tui::start`] here
//! forwards raw input/resize notifications through an `mpsc` channel to a
//! single owning thread, which drives them into [`Tui::process_event`].

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};

use cortexcode_tui_images::{
    delete_kitty_image, get_capabilities, set_cell_dimensions, CellDimensions,
};
use cortexcode_tui_keys::{is_key_release, matches_key};
use cortexcode_tui_terminal::Terminal;
use cortexcode_tui_util::{
    extract_segments, normalize_terminal_output, slice_by_column, slice_with_width, visible_width,
};

use crate::component::{Component, ComponentHandle, Container};
use crate::overlay::{resolve_overlay_layout, OverlayOptions};

/// Cursor position marker: a zero-width APC escape sequence terminals
/// ignore. Components emit this at the cursor position when focused; the
/// TUI finds and strips it, then positions the hardware cursor there.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

const KITTY_SEQUENCE_PREFIX: &str = "\x1b_G";
const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

fn extract_kitty_image_ids(line: &str) -> Vec<u32> {
    let Some(sequence_start) = line.find(KITTY_SEQUENCE_PREFIX) else {
        return Vec::new();
    };
    let params_start = sequence_start + KITTY_SEQUENCE_PREFIX.len();
    let Some(params_end_rel) = line[params_start..].find(';') else {
        return Vec::new();
    };
    let params = &line[params_start..params_start + params_end_rel];
    for param in params.split(',') {
        let mut parts = param.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let Some(value) = parts.next() else { continue };
        if key != "i" {
            continue;
        }
        if let Ok(id) = value.parse::<u64>() {
            if id > 0 && id <= 0xffff_ffff {
                return vec![id as u32];
            }
        }
    }
    Vec::new()
}

/// Result an [`InputListener`] can return to consume or transform input.
#[derive(Default)]
pub struct InputListenerResult {
    pub consume: bool,
    pub data: Option<String>,
}

pub type InputListener = Box<dyn FnMut(&str) -> Option<InputListenerResult>>;

/// Event delivered from the terminal's background threads to the single
/// thread driving the `Tui` (see module docs).
pub enum TuiEvent {
    Input(String),
    Resize,
}

#[derive(Clone, Copy)]
struct CursorPos {
    row: i64,
    col: i64,
}

struct OverlayEntry {
    id: u64,
    component: ComponentHandle,
    options: Option<OverlayOptions>,
    pre_focus: Option<ComponentHandle>,
    hidden: bool,
    focus_order: u64,
}

/// Handle returned by [`Tui::show_overlay`] for controlling the overlay.
/// Unlike the TypeScript original's closures-over-a-shared-entry, this is
/// a plain id passed back into `Tui`'s overlay methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayHandle(u64);

const MIN_LINES_RENDERED_START: i64 = 0;

pub struct Tui {
    pub terminal: Box<dyn Terminal>,
    root: Container,

    previous_lines: Vec<String>,
    previous_width: i64,
    previous_height: i64,
    previous_kitty_image_ids: HashSet<u32>,
    saw_image_line: bool,

    focused_component: Option<ComponentHandle>,
    input_listeners: Vec<InputListener>,

    pub on_debug: Option<Box<dyn FnMut()>>,
    render_requested: bool,

    cursor_row: i64,
    hardware_cursor_row: i64,
    show_hardware_cursor: bool,
    clear_on_shrink: bool,
    max_lines_rendered: i64,
    previous_viewport_top: i64,
    full_redraw_count: u64,
    stopped: bool,

    focus_order_counter: u64,
    overlay_id_counter: u64,
    overlay_stack: Vec<OverlayEntry>,

    last_cursor_pos: Option<CursorPos>,
}

impl Tui {
    pub fn new(terminal: Box<dyn Terminal>, show_hardware_cursor: Option<bool>) -> Self {
        Self {
            terminal,
            root: Container::new(),
            previous_lines: Vec::new(),
            previous_width: 0,
            previous_height: 0,
            previous_kitty_image_ids: HashSet::new(),
            saw_image_line: false,
            focused_component: None,
            input_listeners: Vec::new(),
            on_debug: None,
            render_requested: false,
            cursor_row: 0,
            hardware_cursor_row: 0,
            show_hardware_cursor: show_hardware_cursor.unwrap_or(false),
            clear_on_shrink: false,
            max_lines_rendered: MIN_LINES_RENDERED_START,
            previous_viewport_top: 0,
            full_redraw_count: 0,
            stopped: false,
            focus_order_counter: 0,
            overlay_id_counter: 0,
            overlay_stack: Vec::new(),
            last_cursor_pos: None,
        }
    }

    pub fn full_redraws(&self) -> u64 {
        self.full_redraw_count
    }

    pub fn get_show_hardware_cursor(&self) -> bool {
        self.show_hardware_cursor
    }

    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        if self.show_hardware_cursor == enabled {
            return;
        }
        self.show_hardware_cursor = enabled;
        if !enabled {
            self.terminal.hide_cursor();
        }
        self.request_render(false);
    }

    pub fn get_clear_on_shrink(&self) -> bool {
        self.clear_on_shrink
    }

    pub fn set_clear_on_shrink(&mut self, enabled: bool) {
        self.clear_on_shrink = enabled;
    }

    pub fn add_child(&mut self, component: ComponentHandle) {
        self.root.add_child(component);
    }

    pub fn remove_child(&mut self, component: &ComponentHandle) {
        self.root.remove_child(component);
    }

    pub fn clear_children(&mut self) {
        self.root.clear();
    }

    pub fn set_focus(&mut self, component: Option<ComponentHandle>) {
        if let Some(old) = &self.focused_component {
            if old.borrow().is_focusable() {
                old.borrow_mut().set_focused(false);
            }
        }
        if let Some(new) = &component {
            if new.borrow().is_focusable() {
                new.borrow_mut().set_focused(true);
            }
        }
        self.focused_component = component;
    }

    fn focused_is(&self, component: &ComponentHandle) -> bool {
        matches!(&self.focused_component, Some(c) if Rc::ptr_eq(c, component))
    }

    pub fn show_overlay(
        &mut self,
        component: ComponentHandle,
        options: Option<OverlayOptions>,
    ) -> OverlayHandle {
        self.overlay_id_counter += 1;
        let id = self.overlay_id_counter;
        self.focus_order_counter += 1;
        let non_capturing = options.as_ref().is_some_and(|o| o.non_capturing);
        let entry = OverlayEntry {
            id,
            component: component.clone(),
            options,
            pre_focus: self.focused_component.clone(),
            hidden: false,
            focus_order: self.focus_order_counter,
        };
        let visible = self.is_overlay_visible(&entry);
        self.overlay_stack.push(entry);
        if !non_capturing && visible {
            self.set_focus(Some(component));
        }
        self.terminal.hide_cursor();
        self.request_render(false);
        OverlayHandle(id)
    }

    fn find_overlay_index(&self, handle: OverlayHandle) -> Option<usize> {
        self.overlay_stack.iter().position(|e| e.id == handle.0)
    }

    /// Permanently remove a specific overlay (equivalent to the handle's `hide()`).
    pub fn remove_overlay(&mut self, handle: OverlayHandle) {
        let Some(index) = self.find_overlay_index(handle) else {
            return;
        };
        let entry = self.overlay_stack.remove(index);
        if self.focused_is(&entry.component) {
            let top = self.topmost_visible_overlay_component();
            self.set_focus(top.or(entry.pre_focus));
        }
        if self.overlay_stack.is_empty() {
            self.terminal.hide_cursor();
        }
        self.request_render(false);
    }

    pub fn set_overlay_hidden(&mut self, handle: OverlayHandle, hidden: bool) {
        let Some(index) = self.find_overlay_index(handle) else {
            return;
        };
        if self.overlay_stack[index].hidden == hidden {
            return;
        }
        self.overlay_stack[index].hidden = hidden;
        if hidden {
            let component = self.overlay_stack[index].component.clone();
            if self.focused_is(&component) {
                let top = self.topmost_visible_overlay_component();
                let pre_focus = self.overlay_stack[index].pre_focus.clone();
                self.set_focus(top.or(pre_focus));
            }
        } else {
            let non_capturing = self.overlay_stack[index]
                .options
                .as_ref()
                .is_some_and(|o| o.non_capturing);
            let visible = self.is_overlay_visible(&self.overlay_stack[index]);
            if !non_capturing && visible {
                self.focus_order_counter += 1;
                self.overlay_stack[index].focus_order = self.focus_order_counter;
                let component = self.overlay_stack[index].component.clone();
                self.set_focus(Some(component));
            }
        }
        self.request_render(false);
    }

    pub fn is_overlay_hidden(&self, handle: OverlayHandle) -> bool {
        self.find_overlay_index(handle)
            .map(|i| self.overlay_stack[i].hidden)
            .unwrap_or(true)
    }

    pub fn focus_overlay(&mut self, handle: OverlayHandle) {
        let Some(index) = self.find_overlay_index(handle) else {
            return;
        };
        if !self.is_overlay_visible(&self.overlay_stack[index]) {
            return;
        }
        let component = self.overlay_stack[index].component.clone();
        if !self.focused_is(&component) {
            self.set_focus(Some(component));
        }
        self.focus_order_counter += 1;
        self.overlay_stack[index].focus_order = self.focus_order_counter;
        self.request_render(false);
    }

    pub fn unfocus_overlay(&mut self, handle: OverlayHandle) {
        let Some(index) = self.find_overlay_index(handle) else {
            return;
        };
        let component = self.overlay_stack[index].component.clone();
        if !self.focused_is(&component) {
            return;
        }
        let top = self.topmost_visible_overlay_entry_excluding(index);
        let pre_focus = self.overlay_stack[index].pre_focus.clone();
        self.set_focus(top.or(pre_focus));
        self.request_render(false);
    }

    pub fn is_overlay_focused(&self, handle: OverlayHandle) -> bool {
        self.find_overlay_index(handle)
            .map(|i| self.focused_is(&self.overlay_stack[i].component))
            .unwrap_or(false)
    }

    /// Hide the topmost overlay and restore previous focus.
    pub fn hide_overlay(&mut self) {
        let Some(overlay) = self.overlay_stack.pop() else {
            return;
        };
        if self.focused_is(&overlay.component) {
            let top = self.topmost_visible_overlay_component();
            self.set_focus(top.or(overlay.pre_focus));
        }
        if self.overlay_stack.is_empty() {
            self.terminal.hide_cursor();
        }
        self.request_render(false);
    }

    pub fn has_overlay(&self) -> bool {
        self.overlay_stack
            .iter()
            .any(|e| self.is_overlay_visible(e))
    }

    fn is_overlay_visible(&self, entry: &OverlayEntry) -> bool {
        if entry.hidden {
            return false;
        }
        if let Some(visible_fn) = entry.options.as_ref().and_then(|o| o.visible) {
            return visible_fn(self.terminal.columns(), self.terminal.rows());
        }
        true
    }

    fn topmost_visible_overlay_component(&self) -> Option<ComponentHandle> {
        for entry in self.overlay_stack.iter().rev() {
            if entry.options.as_ref().is_some_and(|o| o.non_capturing) {
                continue;
            }
            if self.is_overlay_visible(entry) {
                return Some(entry.component.clone());
            }
        }
        None
    }

    fn topmost_visible_overlay_entry_excluding(
        &self,
        exclude_index: usize,
    ) -> Option<ComponentHandle> {
        for (i, entry) in self.overlay_stack.iter().enumerate().rev() {
            if i == exclude_index {
                continue;
            }
            if entry.options.as_ref().is_some_and(|o| o.non_capturing) {
                continue;
            }
            if self.is_overlay_visible(entry) {
                return Some(entry.component.clone());
            }
        }
        None
    }

    pub fn invalidate(&mut self) {
        self.root.invalidate();
        for overlay in &self.overlay_stack {
            overlay.component.borrow_mut().invalidate();
        }
    }

    /// Start the terminal and return a channel of raw input/resize events.
    /// The caller must drive these into [`Tui::process_event`] on the
    /// thread that owns this `Tui` (see module docs on the `Send` split).
    pub fn start(&mut self) -> Receiver<TuiEvent> {
        self.stopped = false;
        let (tx, rx) = mpsc::channel();
        let tx_input = tx.clone();
        self.terminal.start(
            Box::new(move |data: &str| {
                let _ = tx_input.send(TuiEvent::Input(data.to_string()));
            }),
            Box::new(move || {
                let _ = tx.send(TuiEvent::Resize);
            }),
        );
        self.terminal.hide_cursor();
        self.query_cell_size();
        self.request_render(false);
        rx
    }

    pub fn process_event(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::Input(data) => self.handle_input(&data),
            TuiEvent::Resize => self.request_render(false),
        }
    }

    pub fn add_input_listener(&mut self, listener: InputListener) {
        self.input_listeners.push(listener);
    }

    fn query_cell_size(&mut self) {
        if get_capabilities().images.is_none() {
            return;
        }
        // Query terminal for cell size in pixels: CSI 16 t.
        // Response: CSI 6 ; height ; width t
        self.terminal.write("\x1b[16t");
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        if !self.previous_lines.is_empty() {
            let target_row = self.previous_lines.len() as i64;
            let line_diff = target_row - self.hardware_cursor_row;
            if line_diff > 0 {
                self.terminal.write(&format!("\x1b[{line_diff}B"));
            } else if line_diff < 0 {
                self.terminal.write(&format!("\x1b[{}A", -line_diff));
            }
            self.terminal.write("\r\n");
        }
        self.terminal.show_cursor();
        self.terminal.stop();
    }

    /// Request a render. `force` clears all cached state for a full
    /// redraw (matching `requestRender(true)`). Unlike the TypeScript
    /// original this renders synchronously and immediately: see the
    /// module-level docs on the dropped 16ms coalescing optimization.
    pub fn request_render(&mut self, force: bool) {
        if force {
            self.previous_lines.clear();
            self.previous_width = -1;
            self.previous_height = -1;
            self.cursor_row = 0;
            self.hardware_cursor_row = 0;
            self.max_lines_rendered = 0;
            self.previous_viewport_top = 0;
        }
        if self.stopped {
            return;
        }
        self.render_requested = true;
        self.render_requested = false;
        self.do_render();
    }

    fn handle_input(&mut self, data: &str) {
        let mut data = data.to_string();
        if !self.input_listeners.is_empty() {
            let mut current = data.clone();
            let mut consumed = false;
            for listener in &mut self.input_listeners {
                if let Some(result) = listener(&current) {
                    if result.consume {
                        consumed = true;
                        break;
                    }
                    if let Some(new_data) = result.data {
                        current = new_data;
                    }
                }
            }
            if consumed {
                return;
            }
            if current.is_empty() {
                return;
            }
            data = current;
        }

        if self.consume_cell_size_response(&data) {
            return;
        }

        if matches_key(&data, "shift+ctrl+d") {
            if let Some(cb) = &mut self.on_debug {
                cb();
                return;
            }
        }

        let focused_overlay_idx = self
            .overlay_stack
            .iter()
            .position(|o| self.focused_is(&o.component));
        if let Some(idx) = focused_overlay_idx {
            if !self.is_overlay_visible(&self.overlay_stack[idx]) {
                let top = self.topmost_visible_overlay_component();
                let pre_focus = self.overlay_stack[idx].pre_focus.clone();
                self.set_focus(top.or(pre_focus));
            }
        }

        if let Some(focused) = self.focused_component.clone() {
            let wants_release = focused.borrow().wants_key_release();
            if is_key_release(&data) && !wants_release {
                return;
            }
            focused.borrow_mut().handle_input(&data);
            self.request_render(false);
        }
    }

    fn consume_cell_size_response(&mut self, data: &str) -> bool {
        // Response format: ESC [ 6 ; height ; width t
        let Some(rest) = data.strip_prefix("\x1b[6;") else {
            return false;
        };
        let Some(rest) = rest.strip_suffix('t') else {
            return false;
        };
        let mut parts = rest.splitn(2, ';');
        let (Some(h), Some(w)) = (parts.next(), parts.next()) else {
            return false;
        };
        let (Ok(height_px), Ok(width_px)) = (h.parse::<u32>(), w.parse::<u32>()) else {
            return false;
        };
        if height_px == 0 || width_px == 0 {
            return true;
        }
        set_cell_dimensions(CellDimensions {
            width_px,
            height_px,
        });
        self.invalidate();
        self.request_render(false);
        true
    }

    /// Composite all visible overlays into content lines (higher focus_order = on top).
    fn composite_overlays(
        &mut self,
        lines: Vec<String>,
        term_width: i64,
        term_height: i64,
    ) -> Vec<String> {
        if self.overlay_stack.is_empty() {
            return lines;
        }
        let mut result = lines;

        struct Rendered {
            lines: Vec<String>,
            row: i64,
            col: i64,
            width: i64,
        }
        let mut rendered = Vec::new();
        let mut min_lines_needed = result.len() as i64;

        let mut visible_indices: Vec<usize> = (0..self.overlay_stack.len())
            .filter(|&i| self.is_overlay_visible(&self.overlay_stack[i]))
            .collect();
        visible_indices.sort_by_key(|&i| self.overlay_stack[i].focus_order);

        for i in visible_indices {
            let (component, options) = {
                let entry = &self.overlay_stack[i];
                (entry.component.clone(), entry.options.clone())
            };
            let layout0 = resolve_overlay_layout(options.as_ref(), 0, term_width, term_height);
            let mut overlay_lines = component.borrow_mut().render(layout0.width.max(0) as u16);
            if let Some(max_height) = layout0.max_height {
                if (overlay_lines.len() as i64) > max_height {
                    overlay_lines.truncate(max_height.max(0) as usize);
                }
            }
            let layout = resolve_overlay_layout(
                options.as_ref(),
                overlay_lines.len() as i64,
                term_width,
                term_height,
            );
            min_lines_needed = min_lines_needed.max(layout.row + overlay_lines.len() as i64);
            rendered.push(Rendered {
                lines: overlay_lines,
                row: layout.row,
                col: layout.col,
                width: layout.width,
            });
        }

        let working_height = (result.len() as i64).max(term_height).max(min_lines_needed);
        while (result.len() as i64) < working_height {
            result.push(String::new());
        }

        let viewport_start = (working_height - term_height).max(0);

        for r in &rendered {
            for (i, overlay_line) in r.lines.iter().enumerate() {
                let idx = viewport_start + r.row + i as i64;
                if idx >= 0 && (idx as usize) < result.len() {
                    let w = r.width.max(0) as usize;
                    let truncated = if visible_width(overlay_line) > w {
                        slice_by_column(overlay_line, 0, w, true)
                    } else {
                        overlay_line.clone()
                    };
                    result[idx as usize] = self.composite_line_at(
                        &result[idx as usize],
                        &truncated,
                        r.col,
                        w as i64,
                        term_width,
                    );
                }
            }
        }

        result
    }

    /// Splice overlay content into a base line at a specific column.
    fn composite_line_at(
        &self,
        base_line: &str,
        overlay_line: &str,
        start_col: i64,
        overlay_width: i64,
        total_width: i64,
    ) -> String {
        if cortexcode_tui_images::is_image_line(base_line) {
            return base_line.to_string();
        }

        let after_start = (start_col + overlay_width).max(0) as usize;
        let after_len = (total_width - after_start as i64).max(0) as usize;
        let (before, before_width, after, after_width) = extract_segments(
            base_line,
            start_col.max(0) as usize,
            after_start,
            after_len,
            true,
        );

        let (overlay_text, overlay_extracted_width) =
            slice_with_width(overlay_line, 0, overlay_width.max(0) as usize, true);

        let before_width = before_width as i64;
        let overlay_extracted_width = overlay_extracted_width as i64;
        let after_width = after_width as i64;

        let before_pad = (start_col - before_width).max(0);
        let overlay_pad = (overlay_width - overlay_extracted_width).max(0);
        let actual_before_width = start_col.max(before_width);
        let actual_overlay_width = overlay_width.max(overlay_extracted_width);
        let after_target = (total_width - actual_before_width - actual_overlay_width).max(0);
        let after_pad = (after_target - after_width).max(0);

        let result = format!(
            "{before}{}{SEGMENT_RESET}{overlay_text}{}{SEGMENT_RESET}{after}{}",
            " ".repeat(before_pad as usize),
            " ".repeat(overlay_pad as usize),
            " ".repeat(after_pad as usize),
        );

        let result_width = visible_width(&result) as i64;
        if result_width <= total_width {
            result
        } else {
            slice_by_column(&result, 0, total_width.max(0) as usize, true)
        }
    }

    fn emit_line(&mut self, line: &str) -> String {
        if cortexcode_tui_images::is_image_line(line) {
            self.saw_image_line = true;
            return line.to_string();
        }
        format!("{}{SEGMENT_RESET}", normalize_terminal_output(line))
    }

    fn collect_kitty_image_ids(&self, lines: &[String]) -> HashSet<u32> {
        if !self.saw_image_line {
            return HashSet::new();
        }
        let mut ids = HashSet::new();
        for line in lines {
            for id in extract_kitty_image_ids(line) {
                ids.insert(id);
            }
        }
        ids
    }

    fn delete_kitty_images(&self, ids: impl IntoIterator<Item = u32>) -> String {
        let mut buffer = String::new();
        for id in ids {
            buffer.push_str(&delete_kitty_image(id));
        }
        buffer
    }

    fn expand_last_changed_for_kitty_images(&self, first_changed: i64, last_changed: i64) -> i64 {
        if !self.saw_image_line {
            return last_changed;
        }
        let mut expanded = last_changed;
        let start = first_changed.max(0) as usize;
        for (i, line) in self.previous_lines.iter().enumerate().skip(start) {
            if !extract_kitty_image_ids(line).is_empty() {
                expanded = expanded.max(i as i64);
            }
        }
        expanded
    }

    fn delete_changed_kitty_images(&self, first_changed: i64, last_changed: i64) -> String {
        if first_changed < 0 || last_changed < first_changed {
            return String::new();
        }
        let mut ids = HashSet::new();
        let max_line = last_changed.min(self.previous_lines.len() as i64 - 1);
        if max_line < 0 {
            return String::new();
        }
        for i in first_changed as usize..=max_line as usize {
            if let Some(line) = self.previous_lines.get(i) {
                for id in extract_kitty_image_ids(line) {
                    ids.insert(id);
                }
            }
        }
        self.delete_kitty_images(ids)
    }

    /// Find and strip the cursor marker from rendered lines, returning its position.
    fn extract_cursor_position(&self, lines: &mut [String], height: i64) -> Option<CursorPos> {
        let viewport_top = (lines.len() as i64 - height).max(0);
        for row in (viewport_top..lines.len() as i64).rev() {
            let line = &lines[row as usize];
            if let Some(marker_index) = line.find(CURSOR_MARKER) {
                let before_marker = &line[..marker_index];
                let col = visible_width(before_marker) as i64;
                let after = line[marker_index + CURSOR_MARKER.len()..].to_string();
                let mut new_line = line[..marker_index].to_string();
                new_line.push_str(&after);
                lines[row as usize] = new_line;
                return Some(CursorPos { row, col });
            }
        }
        None
    }

    /// Flatten the component tree. Simplified relative to the TypeScript
    /// original: always fully re-renders (see module docs on flatten memoization).
    fn render(&mut self, width: u16) -> Vec<String> {
        self.root.render(width)
    }

    fn do_render(&mut self) {
        if self.stopped {
            return;
        }
        let width = self.terminal.columns() as i64;
        let height = self.terminal.rows() as i64;
        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height;
        let previous_buffer_length = if self.previous_height > 0 {
            self.previous_viewport_top + self.previous_height
        } else {
            height
        };
        let mut prev_viewport_top = if height_changed {
            (previous_buffer_length - height).max(0)
        } else {
            self.previous_viewport_top
        };
        let mut viewport_top = prev_viewport_top;
        let mut hardware_cursor_row = self.hardware_cursor_row;

        let mut new_lines = self.render(width.max(0) as u16);

        if !self.overlay_stack.is_empty() {
            new_lines = self.composite_overlays(new_lines, width, height);
        }

        let cursor_pos = self.extract_cursor_position(&mut new_lines, height);
        self.last_cursor_pos = cursor_pos;

        // --- First render: output everything without clearing. ---
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            self.full_render(&new_lines, width, height, false);
            return;
        }

        // --- Width change: always needs a full re-render (wrapping changes). ---
        if width_changed {
            self.full_render(&new_lines, width, height, true);
            return;
        }

        // --- Height change: full re-render to keep the viewport aligned. ---
        if height_changed {
            self.full_render(&new_lines, width, height, true);
            return;
        }

        // --- Content shrunk below the working area: clear empty rows. ---
        if self.clear_on_shrink
            && (new_lines.len() as i64) < self.max_lines_rendered
            && self.overlay_stack.is_empty()
        {
            self.full_render(&new_lines, width, height, true);
            return;
        }

        // --- Full-buffer diff to find first/last changed lines. ---
        let prev_line_count = self.previous_lines.len() as i64;
        let mut first_changed: i64 = -1;
        let mut last_changed: i64 = -1;
        let max_lines = (new_lines.len() as i64).max(prev_line_count);
        for i in 0..max_lines {
            let old_line = if i < prev_line_count {
                self.previous_lines[i as usize].as_str()
            } else {
                ""
            };
            let new_line = if (i as usize) < new_lines.len() {
                new_lines[i as usize].as_str()
            } else {
                ""
            };
            if old_line != new_line {
                if first_changed == -1 {
                    first_changed = i;
                }
                last_changed = i;
            }
        }

        let appended_lines = (new_lines.len() as i64) > prev_line_count;
        if appended_lines {
            if first_changed == -1 {
                first_changed = prev_line_count;
            }
            last_changed = new_lines.len() as i64 - 1;
        }
        if first_changed != -1 {
            last_changed = self.expand_last_changed_for_kitty_images(first_changed, last_changed);
        }
        let append_start = appended_lines && first_changed == prev_line_count && first_changed > 0;

        // --- No changes: still may need to move the hardware cursor. ---
        if first_changed == -1 {
            self.position_hardware_cursor(&cursor_pos, new_lines.len() as i64);
            self.previous_viewport_top = prev_viewport_top;
            self.previous_height = height;
            return;
        }

        // --- All changes are deleted lines: nothing to render, just clear. ---
        if first_changed >= new_lines.len() as i64 {
            if prev_line_count > new_lines.len() as i64 {
                let mut buffer = String::from("\x1b[?2026h");
                buffer.push_str(&self.delete_changed_kitty_images(first_changed, last_changed));
                let target_row = (new_lines.len() as i64 - 1).max(0);
                if target_row < prev_viewport_top {
                    self.full_render(&new_lines, width, height, true);
                    return;
                }
                let line_diff = {
                    let current_screen_row = hardware_cursor_row - prev_viewport_top;
                    let target_screen_row = target_row - viewport_top;
                    target_screen_row - current_screen_row
                };
                if line_diff > 0 {
                    buffer.push_str(&format!("\x1b[{line_diff}B"));
                } else if line_diff < 0 {
                    buffer.push_str(&format!("\x1b[{}A", -line_diff));
                }
                buffer.push('\r');
                let extra_lines = prev_line_count - new_lines.len() as i64;
                if extra_lines > height {
                    self.full_render(&new_lines, width, height, true);
                    return;
                }
                if extra_lines > 0 {
                    buffer.push_str("\x1b[1B");
                }
                for i in 0..extra_lines {
                    buffer.push_str("\r\x1b[2K");
                    if i < extra_lines - 1 {
                        buffer.push_str("\x1b[1B");
                    }
                }
                if extra_lines > 0 {
                    buffer.push_str(&format!("\x1b[{extra_lines}A"));
                }
                buffer.push_str("\x1b[?2026l");
                self.terminal.write(&buffer);
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(&cursor_pos, new_lines.len() as i64);
            self.previous_kitty_image_ids = self.collect_kitty_image_ids(&new_lines);
            self.previous_lines = new_lines;
            self.previous_width = width;
            self.previous_height = height;
            self.previous_viewport_top = prev_viewport_top;
            return;
        }

        // --- Changes above the previous viewport require a full redraw. ---
        if first_changed < prev_viewport_top {
            self.full_render(&new_lines, width, height, true);
            return;
        }

        // --- Differential render from first changed line to end. ---
        let mut buffer = String::from("\x1b[?2026h");
        buffer.push_str(&self.delete_changed_kitty_images(first_changed, last_changed));
        let prev_viewport_bottom = prev_viewport_top + height - 1;
        let move_target_row = if append_start {
            first_changed - 1
        } else {
            first_changed
        };
        if move_target_row > prev_viewport_bottom {
            let current_screen_row = (hardware_cursor_row - prev_viewport_top).clamp(0, height - 1);
            let move_to_bottom = height - 1 - current_screen_row;
            if move_to_bottom > 0 {
                buffer.push_str(&format!("\x1b[{move_to_bottom}B"));
            }
            let scroll = move_target_row - prev_viewport_bottom;
            buffer.push_str(&"\r\n".repeat(scroll.max(0) as usize));
            prev_viewport_top += scroll;
            viewport_top += scroll;
            hardware_cursor_row = move_target_row;
        }

        let line_diff = {
            let current_screen_row = hardware_cursor_row - prev_viewport_top;
            let target_screen_row = move_target_row - viewport_top;
            target_screen_row - current_screen_row
        };
        if line_diff > 0 {
            buffer.push_str(&format!("\x1b[{line_diff}B"));
        } else if line_diff < 0 {
            buffer.push_str(&format!("\x1b[{}A", -line_diff));
        }

        buffer.push_str(if append_start { "\r\n" } else { "\r" });

        let render_end = last_changed.min(new_lines.len() as i64 - 1);
        for i in first_changed..=render_end {
            if i > first_changed {
                buffer.push_str("\r\n");
            }
            buffer.push_str("\x1b[2K");
            let line = &new_lines[i as usize];
            let is_image = cortexcode_tui_images::is_image_line(line);
            if !is_image && visible_width(line) as i64 > width {
                // The original crashes with a debug log here; we surface a
                // plain error since this indicates a component bug (a line
                // wider than the terminal that failed to truncate itself).
                self.stop();
                panic!(
                    "Rendered line {i} exceeds terminal width ({} > {width}). \
                     A custom TUI component is not truncating its output; \
                     use visible_width()/truncate_to_width().",
                    visible_width(line)
                );
            }
            if is_image {
                self.saw_image_line = true;
            }
            buffer.push_str(
                if is_image {
                    line.clone()
                } else {
                    format!("{}{SEGMENT_RESET}", normalize_terminal_output(line))
                }
                .as_str(),
            );
        }

        let mut final_cursor_row = render_end;
        if prev_line_count > new_lines.len() as i64 {
            if render_end < new_lines.len() as i64 - 1 {
                let move_down = new_lines.len() as i64 - 1 - render_end;
                buffer.push_str(&format!("\x1b[{move_down}B"));
                final_cursor_row = new_lines.len() as i64 - 1;
            }
            let extra_lines = prev_line_count - new_lines.len() as i64;
            for _ in new_lines.len() as i64..prev_line_count {
                buffer.push_str("\r\n\x1b[2K");
            }
            buffer.push_str(&format!("\x1b[{extra_lines}A"));
        }

        buffer.push_str("\x1b[?2026l");
        self.terminal.write(&buffer);

        self.cursor_row = (new_lines.len() as i64 - 1).max(0);
        self.hardware_cursor_row = final_cursor_row;
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len() as i64);
        self.previous_viewport_top = prev_viewport_top.max(final_cursor_row - height + 1);

        self.position_hardware_cursor(&cursor_pos, new_lines.len() as i64);

        self.previous_kitty_image_ids = self.collect_kitty_image_ids(&new_lines);
        self.previous_lines = new_lines;
        self.previous_width = width;
        self.previous_height = height;
    }

    fn full_render(&mut self, new_lines: &[String], width: i64, height: i64, clear: bool) {
        self.full_redraw_count += 1;
        let mut buffer = String::from("\x1b[?2026h");
        if clear {
            let ids: Vec<u32> = self.previous_kitty_image_ids.iter().copied().collect();
            buffer.push_str(&self.delete_kitty_images(ids));
            buffer.push_str("\x1b[2J\x1b[H\x1b[3J");
        }
        for (i, line) in new_lines.iter().enumerate() {
            if i > 0 {
                buffer.push_str("\r\n");
            }
            buffer.push_str(&self.emit_line(line));
        }
        buffer.push_str("\x1b[?2026l");
        self.terminal.write(&buffer);

        self.cursor_row = (new_lines.len() as i64 - 1).max(0);
        self.hardware_cursor_row = self.cursor_row;
        if clear {
            self.max_lines_rendered = new_lines.len() as i64;
        } else {
            self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len() as i64);
        }
        let buffer_length = height.max(new_lines.len() as i64);
        self.previous_viewport_top = (buffer_length - height).max(0);

        let cp = self.last_cursor_pos;
        self.position_hardware_cursor(&cp, new_lines.len() as i64);

        self.previous_lines = new_lines.to_vec();
        self.previous_kitty_image_ids = self.collect_kitty_image_ids(new_lines);
        self.previous_width = width;
        self.previous_height = height;
    }

    fn position_hardware_cursor(&mut self, cursor_pos: &Option<CursorPos>, total_lines: i64) {
        let Some(cursor_pos) = cursor_pos else {
            self.terminal.hide_cursor();
            return;
        };
        if total_lines <= 0 {
            self.terminal.hide_cursor();
            return;
        }

        let target_row = cursor_pos.row.clamp(0, total_lines - 1);
        let target_col = cursor_pos.col.max(0);

        let row_delta = target_row - self.hardware_cursor_row;
        let mut buffer = String::new();
        if row_delta > 0 {
            buffer.push_str(&format!("\x1b[{row_delta}B"));
        } else if row_delta < 0 {
            buffer.push_str(&format!("\x1b[{}A", -row_delta));
        }
        buffer.push_str(&format!("\x1b[{}G", target_col + 1));

        if !buffer.is_empty() {
            self.terminal.write(&buffer);
        }

        self.hardware_cursor_row = target_row;
        if self.show_hardware_cursor {
            self.terminal.show_cursor();
        } else {
            self.terminal.hide_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{OverlayAnchor, SizeValue};
    use std::cell::RefCell as StdRefCell;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct SharedLog(Arc<Mutex<Vec<String>>>);

    impl SharedLog {
        fn push(&self, s: &str) {
            self.0.lock().unwrap().push(s.to_string());
        }
        fn joined(&self) -> String {
            self.0.lock().unwrap().join("")
        }
        fn clear(&self) {
            self.0.lock().unwrap().clear();
        }
    }

    struct MockTerminal {
        writes: SharedLog,
        cols: Arc<Mutex<u16>>,
        rows: Arc<Mutex<u16>>,
        hide_cursor_calls: Arc<Mutex<u32>>,
        show_cursor_calls: Arc<Mutex<u32>>,
    }

    impl MockTerminal {
        fn new(cols: u16, rows: u16) -> Self {
            Self {
                writes: SharedLog::default(),
                cols: Arc::new(Mutex::new(cols)),
                rows: Arc::new(Mutex::new(rows)),
                hide_cursor_calls: Arc::new(Mutex::new(0)),
                show_cursor_calls: Arc::new(Mutex::new(0)),
            }
        }
    }

    impl Terminal for MockTerminal {
        fn start(
            &mut self,
            _on_input: Box<dyn FnMut(&str) + Send>,
            _on_resize: Box<dyn FnMut() + Send>,
        ) {
        }
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max: Duration, _idle: Duration) {}
        fn write(&mut self, data: &str) {
            self.writes.push(data);
        }
        fn columns(&self) -> u16 {
            *self.cols.lock().unwrap()
        }
        fn rows(&self) -> u16 {
            *self.rows.lock().unwrap()
        }
        fn kitty_protocol_active(&self) -> bool {
            false
        }
        fn move_by(&mut self, _lines: i32) {}
        fn hide_cursor(&mut self) {
            *self.hide_cursor_calls.lock().unwrap() += 1;
        }
        fn show_cursor(&mut self) {
            *self.show_cursor_calls.lock().unwrap() += 1;
        }
        fn clear_line(&mut self) {}
        fn clear_from_cursor(&mut self) {}
        fn clear_screen(&mut self) {}
        fn set_title(&mut self, _title: &str) {}
        fn set_progress(&mut self, _active: bool) {}
    }

    struct TestComponent {
        lines: Vec<String>,
        focused: bool,
        focusable: bool,
    }

    impl TestComponent {
        fn new() -> Rc<StdRefCell<Self>> {
            Rc::new(StdRefCell::new(Self {
                lines: Vec::new(),
                focused: false,
                focusable: false,
            }))
        }
    }

    impl Component for TestComponent {
        fn render(&mut self, _width: u16) -> Vec<String> {
            self.lines.clone()
        }
        fn is_focusable(&self) -> bool {
            self.focusable
        }
        fn set_focused(&mut self, focused: bool) {
            self.focused = focused;
        }
    }

    struct MockHandles {
        writes: SharedLog,
        cols: Arc<Mutex<u16>>,
        rows: Arc<Mutex<u16>>,
    }

    impl MockHandles {
        fn set_size(&self, cols: u16, rows: u16) {
            *self.cols.lock().unwrap() = cols;
            *self.rows.lock().unwrap() = rows;
        }
    }

    fn new_tui(cols: u16, rows: u16) -> (Tui, MockHandles) {
        let terminal = MockTerminal::new(cols, rows);
        let handles = MockHandles {
            writes: terminal.writes.clone(),
            cols: terminal.cols.clone(),
            rows: terminal.rows.clone(),
        };
        (Tui::new(Box::new(terminal), Some(false)), handles)
    }

    #[test]
    fn first_render_writes_all_lines_without_clearing() {
        let (mut tui, handles) = new_tui(40, 10);
        let component = TestComponent::new();
        component.borrow_mut().lines = vec!["a".to_string(), "b".to_string()];
        tui.add_child(component);

        tui.request_render(false);

        let out = handles.writes.joined();
        assert!(out.contains("a"));
        assert!(out.contains("b"));
        assert!(
            !out.contains("\x1b[2J"),
            "first render should not clear the screen"
        );
        assert!(out.starts_with("\x1b[?2026h"));
        assert!(out.ends_with("\x1b[?2026l"));
    }

    #[test]
    fn no_change_produces_no_writes() {
        let (mut tui, handles) = new_tui(40, 10);
        let component = TestComponent::new();
        component.borrow_mut().lines = vec!["a".to_string()];
        tui.add_child(component);

        tui.request_render(false);
        handles.writes.clear();

        tui.request_render(false);
        assert_eq!(handles.writes.joined(), "");
    }

    #[test]
    fn width_change_forces_full_clear() {
        let (mut tui, handles) = new_tui(40, 10);
        let component = TestComponent::new();
        component.borrow_mut().lines = vec!["a".to_string()];
        tui.add_child(component);
        tui.request_render(false);
        handles.writes.clear();

        handles.set_size(80, 10);
        tui.request_render(false);
        assert!(
            handles.writes.joined().contains("\x1b[2J"),
            "width change should force full clear"
        );
    }

    #[test]
    fn appended_lines_are_written_without_full_clear() {
        let (mut tui, handles) = new_tui(40, 10);
        let component = TestComponent::new();
        component.borrow_mut().lines = vec!["a".to_string()];
        tui.add_child(component.clone());
        tui.request_render(false);
        handles.writes.clear();

        component.borrow_mut().lines = vec!["a".to_string(), "b".to_string()];
        tui.request_render(false);

        let out = handles.writes.joined();
        assert!(out.contains('b'));
        assert!(!out.contains("\x1b[2J"));
    }

    #[test]
    fn shrinking_content_clears_deleted_lines() {
        let (mut tui, handles) = new_tui(40, 10);
        let component = TestComponent::new();
        component.borrow_mut().lines = vec!["a".to_string(), "b".to_string()];
        tui.add_child(component.clone());
        tui.request_render(false);
        handles.writes.clear();

        component.borrow_mut().lines = vec!["a".to_string()];
        tui.request_render(false);

        let out = handles.writes.joined();
        assert!(out.contains("\x1b[2K"), "deleted line should be cleared");
        assert!(
            !out.contains("\x1b[2J"),
            "shrink-by-default should not force a full screen clear"
        );
    }

    #[test]
    fn deletes_changed_kitty_image_before_drawing_new_placement() {
        use cortexcode_tui_images::{delete_kitty_image, encode_kitty, KittyEncodeOptions};

        let (mut tui, handles) = new_tui(40, 10);
        let component = TestComponent::new();
        let old_image = encode_kitty(
            "AAAA",
            &KittyEncodeOptions {
                columns: Some(2),
                rows: Some(2),
                image_id: Some(42),
                move_cursor: Some(false),
            },
        );
        component.borrow_mut().lines = vec!["top".to_string(), old_image];
        tui.add_child(component.clone());
        tui.request_render(false);
        handles.writes.clear();

        let new_image = encode_kitty(
            "BBBB",
            &KittyEncodeOptions {
                columns: Some(2),
                rows: Some(1),
                image_id: Some(42),
                move_cursor: Some(false),
            },
        );
        component.borrow_mut().lines = vec![new_image.clone(), String::new()];
        tui.request_render(false);

        let out = handles.writes.joined();
        let delete_seq = delete_kitty_image(42);
        let delete_idx = out.find(&delete_seq);
        let draw_idx = out.find(&new_image);
        assert!(delete_idx.is_some(), "changed old image should be deleted");
        assert!(draw_idx.is_some(), "new image should be drawn");
        assert!(
            delete_idx.unwrap() < draw_idx.unwrap(),
            "old image must be deleted before the new placement is drawn"
        );
    }

    #[test]
    fn cursor_marker_is_stripped_and_positions_hardware_cursor() {
        let (mut tui, handles) = new_tui(40, 10);
        tui.set_show_hardware_cursor(true);
        let component = TestComponent::new();
        component.borrow_mut().lines = vec![format!("hello{CURSOR_MARKER}")];
        tui.add_child(component);

        tui.request_render(false);

        let out = handles.writes.joined();
        assert!(
            !out.contains(CURSOR_MARKER),
            "marker should be stripped from output"
        );
        assert!(out.contains("hello"));
        // Column move to col 6 (1-indexed) after "hello".
        assert!(out.contains("\x1b[6G"));
    }

    #[test]
    fn overlay_composites_over_base_content() {
        let (mut tui, handles) = new_tui(20, 5);
        let base = TestComponent::new();
        base.borrow_mut().lines = vec!["0123456789012345678901234567890".to_string(); 3];
        tui.add_child(base);

        let overlay = TestComponent::new();
        overlay.borrow_mut().lines = vec!["OVERLAY".to_string()];
        let options = OverlayOptions {
            anchor: Some(OverlayAnchor::TopLeft),
            width: Some(SizeValue::Absolute(7)),
            ..Default::default()
        };
        tui.show_overlay(overlay, Some(options));

        tui.request_render(false);
        let out = handles.writes.joined();
        assert!(out.contains("OVERLAY"));
    }

    #[test]
    fn full_redraw_count_increments_only_on_full_renders() {
        let (mut tui, _writes) = new_tui(40, 10);
        let component = TestComponent::new();
        component.borrow_mut().lines = vec!["a".to_string()];
        tui.add_child(component.clone());
        tui.request_render(false);
        assert_eq!(tui.full_redraws(), 1);

        component.borrow_mut().lines = vec!["a".to_string(), "b".to_string()];
        tui.request_render(false);
        assert_eq!(
            tui.full_redraws(),
            1,
            "appending lines should not trigger a full redraw"
        );
    }
}
