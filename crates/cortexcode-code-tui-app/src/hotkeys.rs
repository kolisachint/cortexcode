//! `/hotkeys`: the keyboard shortcuts page, hoocode `handleHotkeys`.

use cortexcode_code_tui_keybindings::{key_display_label, key_display_text};

/// The shortcuts page as markdown, with the effective key for every binding.
/// Extension shortcuts join it with the extension runner (12.3).
pub fn hotkeys_markdown() -> String {
    let submit = key_display_text("tui.input.submit");
    let new_line = key_display_text("tui.input.newLine");
    let tab = key_display_text("tui.input.tab");
    let external_editor = key_display_text("app.editor.external");
    let voice = key_display_text("app.input.voiceTranscribe");
    let paste_image = key_display_text("app.clipboard.pasteImage");
    let copy_message = key_display_text("app.clipboard.copyMessage");
    let follow_up = key_display_text("app.message.followUp");
    let dequeue = key_display_text("app.message.dequeue");
    let cycle_mode = key_display_label("app.mode.cycleForward");
    let cycle_mode_back = key_display_label("app.mode.cycleBackward");
    let cycle_model_forward = key_display_label("app.model.cycleForward");
    let cycle_model_backward = key_display_label("app.model.cycleBackward");
    let cycle_thinking_level = key_display_label("app.thinking.cycleForward");
    let cycle_thinking_level_back = key_display_label("app.thinking.cycleBackward");
    let view_forward = key_display_label("app.view.cycleForward");
    let view_backward = key_display_label("app.view.cycleBackward");
    let cycle_task_view = key_display_label("app.tasks.cycleForward");
    let cycle_task_view_back = key_display_label("app.tasks.cycleBackward");
    let expand_tools = key_display_text("app.tools.expand");
    let toggle_thinking = key_display_text("app.thinking.toggle");
    let team_focus = key_display_text("app.team.focus");
    let team_nudge = key_display_text("app.team.nudge");
    let team_attach = key_display_text("app.team.attach");
    let interrupt = key_display_text("app.interrupt");
    let chrome_forward = key_display_label("app.chrome.cycleForward");
    let chrome_backward = key_display_label("app.chrome.cycleBackward");
    let scroll_page_up = key_display_text("app.scroll.pageUp");
    let scroll_page_down = key_display_text("app.scroll.pageDown");
    let scroll_top = key_display_text("app.scroll.top");
    let scroll_bottom = key_display_text("app.scroll.bottom");
    let scroll_line_up = key_display_text("app.scroll.lineUp");
    let scroll_line_down = key_display_text("app.scroll.lineDown");
    let scroll_previous_message = key_display_text("app.scroll.previousMessage");
    let scroll_next_message = key_display_text("app.scroll.nextMessage");
    let scroll_search = key_display_text("app.scroll.search");
    let scroll_search_in_view = key_display_text("app.scroll.searchInView");
    let scroll_search_next = key_display_text("app.scroll.searchNext");
    let scroll_search_previous = key_display_text("app.scroll.searchPrevious");
    let scroll_exit = key_display_text("app.scroll.exit");
    let session_resume = key_display_text("app.session.resume");
    let change_directory = key_display_text("app.session.changeDirectory");
    let cycle_session_color = key_display_label("app.session.color.cycleForward");
    let cycle_session_color_backward = key_display_label("app.session.color.cycleBackward");
    let open_settings = key_display_text("app.settings.open");
    let open_hotkeys = key_display_text("app.hotkeys.open");
    let clear = key_display_text("app.clear");
    let exit = key_display_text("app.exit");
    let suspend = key_display_text("app.suspend");
    let cursor_up = key_display_text("tui.editor.cursorUp");
    let cursor_down = key_display_text("tui.editor.cursorDown");
    let cursor_left = key_display_text("tui.editor.cursorLeft");
    let cursor_right = key_display_text("tui.editor.cursorRight");
    let cursor_word_left = key_display_text("tui.editor.cursorWordLeft");
    let cursor_word_right = key_display_text("tui.editor.cursorWordRight");
    let cursor_line_start = key_display_text("tui.editor.cursorLineStart");
    let cursor_line_end = key_display_text("tui.editor.cursorLineEnd");
    let jump_forward = key_display_text("tui.editor.jumpForward");
    let jump_backward = key_display_text("tui.editor.jumpBackward");
    let delete_word_backward = key_display_text("tui.editor.deleteWordBackward");
    let delete_word_forward = key_display_text("tui.editor.deleteWordForward");
    let delete_to_line_start = key_display_text("tui.editor.deleteToLineStart");
    let delete_to_line_end = key_display_text("tui.editor.deleteToLineEnd");
    let yank = key_display_text("tui.editor.yank");
    let yank_pop = key_display_text("tui.editor.yankPop");
    let undo = key_display_text("tui.editor.undo");
    let redo = key_display_text("tui.editor.redo");
    let win = if cfg!(windows) {
        " (Ctrl+Enter on Windows Terminal)"
    } else {
        ""
    };
    format!(
        r#"
Grouped by what you are doing, not by what the key is. Seven groups, none of the
learned ones bigger than five — the size a person can actually hold. Three are
free: **Flow** is what every terminal program already taught you, **Scroll** is
what every pager did, and every picker prints its own keys on its own hint line,
so you read those instead of remembering them. **Screen** is one key.

**Compose** — the message in your hands
| Key | Action |
|-----|--------|
| `{submit}` | Send message |
| `{new_line}` | New line{win} |
| `{tab}` | Path completion / accept autocomplete |
| `{external_editor}` | Edit the message in `$VISUAL` / `$EDITOR` |
| `{voice}` | Speak instead of type |
| `{paste_image}` | Paste image from clipboard |
| `{copy_message}` | Copy the agent's last message (`/copy`; `/copy all` for the session) |
| `{follow_up}` | Queue a follow-up while the agent works |
| `{dequeue}` | Bring every queued message back to the editor |
| `/` `!` `!!` | Slash commands · run bash · run bash off the record |

**Steer** — what the agent is before it runs
The only three keys that change what happens next, and the only three that cost
anything. Step forward on the first key, back on the second; the footer shows
all three.

| Key | Steps | Through |
|-----|-------|---------|
| `{cycle_mode}` / `{cycle_mode_back}` | Agent mode | ask → plan → build → debug (`/mode` picks one) |
| `{cycle_model_forward}` / `{cycle_model_backward}` | Model | your enabled models (`/model` picks one) |
| `{cycle_thinking_level}` / `{cycle_thinking_level_back}` | Thinking level | off → … → high |

**Read** — what you see of what it did
Free and reversible, every one: nothing here touches the work, only the window
onto it. Press again or add `Shift` and you are back where you were.

| Key | Action |
|-----|--------|
| `{view_forward}` / `{view_backward}` | Step tool output: radar → peek → full |
| `{cycle_task_view}` / `{cycle_task_view_back}` | Step the task panel: tasks → subagents → teams |
| `{expand_tools}` | Jump to the full view and back, without moving the dial |
| `{toggle_thinking}` | Show or hide thinking blocks |
| `{team_focus}` | Focus the team roster — `{team_nudge}` nudges, `{team_attach}` attaches, `q`/`{interrupt}` leaves (`--team`) |

**Screen** — how much room there is to see it in
One dial for the furniture. Everything below the transcript except the prompt,
which never hides.

| Key | Steps | Through |
|-----|-------|---------|
| `{chrome_forward}` / `{chrome_backward}` | Chrome | full → compact (one-row footer, no task list) → bare (neither) — `/chrome` picks one |

It also gets out of the way on its own: the footer lends its rows to the
completion list while that is open, and the task list keeps its counts but drops
its rows while the agent is mid-turn. Both come back by themselves.

**Scroll** — where in the session you are looking
The wheel and these keys move the same view, and once it is scrolled back it
**stays** there: output keeps arriving underneath without moving what you are
reading. The bottom row tells you where you are and how to get back.

| Key | Action |
|-----|--------|
| `{scroll_page_up}` / `{scroll_page_down}` | Scroll a page — `{scroll_page_up}` is also how you start |
| `{scroll_top}` / `{scroll_bottom}` | Jump to the start of the session / back to live |
| `{scroll_line_up}` / `{scroll_line_down}` | A line at a time (while scrolled back) |
| `{scroll_previous_message}` / `{scroll_next_message}` | Jump to your previous / next message |
| `{scroll_search}` | Search the transcript — `{scroll_search_in_view}` once you are already scrolled back |
| `{scroll_search_next}` / `{scroll_search_previous}` | Step the matches, once the query is committed with `{submit}` |
| `{scroll_exit}` | Back to live output |
| Wheel | Three lines a notch, wherever you turn it |

Search runs backwards, like `{scroll_search}` in a shell: the newest match first,
because what you are looking for in a session is behind you.

Scrolling to the bottom hands the live view back on its own, and so does typing:
any key that is not one of these returns you to live and then does its usual job.

**Go** — sessions and places
Each takes the screen and hands it back on `{interrupt}`, and each has a slash
command that does the same thing.

| Key | Action |
|-----|--------|
| `{session_resume}` | Resume a session from history (`/resume`) |
| `{change_directory}` | Change working directory (`/cd`) |
| `{cycle_session_color}` / `{cycle_session_color_backward}` | Step the session chip's color (`/color` picks one) |
| `{open_settings}` | Open settings (`/settings`) |
| `{open_hotkeys}` | Show this list (`/hotkeys`) |
| `/tree` | Open the session tree |

**Flow** — getting out, getting back
| Key | Action |
|-----|--------|
| `{interrupt}` | Cancel autocomplete / abort streaming |
| `{clear}` | Clear editor (first) / exit (second) |
| `{exit}` | Exit when the editor is empty |
| `{suspend}` | Suspend to background |

### What the chord tells you

`Alt`+letter **sets a value** and nothing takes the screen — you keep typing.
Seven of those are dials, and `Shift` always steps one back: **a**gent mode,
**m**odel, **t**hinking, tool **o**utput, task **l**ist, session **c**olor, and
chrome (**z**, the one letter that names nothing — its stop is the shape of the
screen in front of you).

`Ctrl`+letter **acts on what is drawn right now** and shares its letter with the
`Alt` key for the same subject. `{view_forward}` sets how much tool output there
ever is, `{expand_tools}` jumps to all of it and back; `{cycle_thinking_level}`
sets how much thinking there ever is, `{toggle_thinking}` shows or hides what you
have. Two subjects, two letters, four keys.

The thinking level also answers to `Shift+Tab` — it is the one dial with no
slash command, so that is the way to it on a terminal that does not send `Alt`.

Inside a picker every `Ctrl` key still edits the query, so the picker's own verbs
are on `Alt` and its hint line names them.

### The editor
Standard readline/emacs bindings — reference, not something to memorise.
`{scroll_page_up}` / `{scroll_page_down}` are **not** here: they scroll the session,
not the prompt, which is one to three lines almost every time you look at it.

| Key | Action |
|-----|--------|
| `{cursor_up}` / `{cursor_down}` / `{cursor_left}` / `{cursor_right}` | Move cursor / browse history (Up when empty) |
| `{cursor_word_left}` / `{cursor_word_right}` | Move by word |
| `{cursor_line_start}` / `{cursor_line_end}` | Start / end of line |
| `{jump_forward}` / `{jump_backward}` | Jump forward / backward to character |
| `{delete_word_backward}` / `{delete_word_forward}` | Delete word backwards / forwards |
| `{delete_to_line_start}` / `{delete_to_line_end}` | Delete to start / end of line |
| `{yank}` / `{yank_pop}` | Paste the most-recently-deleted text / cycle older ones |
| `{undo}` / `{redo}` | Undo / redo |
"#
    )
    .trim()
    .to_string()
}
