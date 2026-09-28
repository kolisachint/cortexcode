//! `cli/session-picker.ts`: the session selector on its own TUI, for
//! `--resume` before the session (and the interactive mode) exists.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use cortexcode_code_agent_session::runtime::{format_missing_session_cwd_prompt, SessionCwdIssue};
use cortexcode_code_resources::package_resolve::ResolvedPaths;
use cortexcode_code_session::{list_all_sessions, list_sessions, SessionInfo, SessionListProgress};
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tui_keybindings::AppKeybindingsManager;
use cortexcode_code_tui_selectors::config_selector::ConfigSelectorComponent;
use cortexcode_code_tui_selectors::session_selector::{
    LoadSink, SessionSelectorComponent, SessionSelectorOptions, SessionsLoader,
};
use cortexcode_tui_render::{Component, ComponentHandle, Tui};

use crate::extension_selector::{ExtensionSelectorComponent, SelectorOutcome};
use cortexcode_tui_terminal::Terminal;

/// A loader that lists on a worker thread and reports through the sink.
fn threaded_loader<F>(list: F) -> SessionsLoader
where
    F: Fn(&SessionListProgress) -> Result<Vec<SessionInfo>, String> + Send + Clone + 'static,
{
    Box::new(move |sink: LoadSink| {
        let list = list.clone();
        std::thread::spawn(move || {
            let progress_sink = sink.clone();
            let progress: Box<SessionListProgress> =
                Box::new(move |loaded, total| progress_sink.progress(loaded, total));
            sink.finish(list(progress.as_ref()));
        });
    })
}

/// The current folder's sessions (`SessionManager.list(cwd, sessionDir)`).
pub fn current_sessions_loader(session_dir: PathBuf) -> SessionsLoader {
    threaded_loader(move |progress| {
        list_sessions(&session_dir, Some(progress)).map_err(|e| e.to_string())
    })
}

/// Every project's sessions (`SessionManager.listAll`).
pub fn all_sessions_loader() -> SessionsLoader {
    threaded_loader(|progress| list_all_sessions(Some(progress)).map_err(|e| e.to_string()))
}

/// Run `component` on its own TUI until `done` holds an answer (`None` when
/// the terminal's input closes first).
fn run_standalone<T, C: Component + 'static>(
    component: Rc<RefCell<C>>,
    mut tui: Tui,
    done: Rc<RefCell<Option<T>>>,
    mut poll: impl FnMut(&mut C) -> bool,
) -> Option<T> {
    let handle: ComponentHandle = component.clone();
    tui.add_child(handle.clone());
    tui.set_focus(Some(handle));
    let input = tui.start();
    loop {
        match input.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                tui.process_event(event);
                while let Ok(event) = input.try_recv() {
                    tui.process_event(event);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if poll(&mut component.borrow_mut()) {
            tui.request_render(false);
        }
        if done.borrow().is_some() {
            break;
        }
    }
    tui.stop();
    done.take()
}

fn process_tui(terminal: Option<Box<dyn Terminal>>, show_hardware_cursor: Option<bool>) -> Tui {
    let terminal =
        terminal.unwrap_or_else(|| Box::new(cortexcode_tui_terminal::ProcessTerminal::new()));
    Tui::new(terminal, show_hardware_cursor)
}

/// `selectSession`: the chosen session file, or `None` when cancelled.
pub fn select_session(
    current_loader: SessionsLoader,
    all_loader: SessionsLoader,
    terminal: Option<Box<dyn Terminal>>,
) -> Option<PathBuf> {
    let tui = process_tui(terminal, None);
    let keybindings = AppKeybindingsManager::create(None);
    keybindings.install();

    let outcome: Rc<RefCell<Option<Option<PathBuf>>>> = Rc::default();
    let on_select = outcome.clone();
    let on_cancel = outcome.clone();
    // The pin focuses the list; the selector hands every key to it here.
    let selector = Rc::new(RefCell::new(SessionSelectorComponent::new(
        current_loader,
        all_loader,
        Box::new(move |path| {
            on_select.borrow_mut().get_or_insert(Some(path));
        }),
        Box::new(move || {
            on_cancel.borrow_mut().get_or_insert(None);
        }),
        SessionSelectorOptions {
            show_rename_hint: Some(false),
            keybindings: Some(Rc::new(keybindings.into_manager())),
            ..Default::default()
        },
        None,
    )));
    run_standalone(selector, tui, outcome, |s| s.poll()).flatten()
}

/// main.ts `promptForMissingSessionCwd`: Continue in the fallback cwd, or
/// cancel (`None`).
pub fn prompt_for_missing_session_cwd(
    issue: &SessionCwdIssue,
    theme_name: Option<&str>,
    show_hardware_cursor: bool,
    clear_on_shrink: bool,
) -> Option<String> {
    cortexcode_code_tui_theme::init_theme(theme_name, false);
    AppKeybindingsManager::create(None).install();
    let mut tui = process_tui(None, Some(show_hardware_cursor));
    tui.set_clear_on_shrink(clear_on_shrink);

    let outcome: Rc<RefCell<Option<Option<String>>>> = Rc::default();
    let sink = outcome.clone();
    let fallback = issue.fallback_cwd.clone();
    let selector = Rc::new(RefCell::new(ExtensionSelectorComponent::new(
        &format_missing_session_cwd_prompt(issue),
        vec!["Continue".to_string(), "Cancel".to_string()],
        None,
        Box::new(move |choice| {
            let answer = match choice {
                SelectorOutcome::Selected(option) if option == "Continue" => Some(fallback.clone()),
                _ => None,
            };
            sink.borrow_mut().get_or_insert(answer);
        }),
    )));
    run_standalone(selector, tui, outcome, |s| s.poll()).flatten()
}

/// main.ts's `--resume` branch: the settings' theme (watched while the picker
/// is up), then the picker over this folder's and every folder's sessions.
/// `cli/config-selector.ts` `selectConfig`: the resource list on its own TUI
/// until escape. Returns `false` when ctrl+c asked to exit the process
/// (`process.exit(0)` in the original).
pub fn select_config(
    resolved: &ResolvedPaths,
    settings: Rc<RefCell<SettingsManager>>,
    cwd: &str,
    agent_dir: &str,
) -> bool {
    let theme_name = settings.borrow().theme();
    cortexcode_code_tui_theme::init_theme(theme_name.as_deref(), true);
    AppKeybindingsManager::create(None).install();
    let tui = process_tui(None, None);
    // Some(true) = closed, Some(false) = exit.
    let outcome: Rc<RefCell<Option<bool>>> = Rc::default();
    let on_close = outcome.clone();
    let on_exit = outcome.clone();
    let selector = Rc::new(RefCell::new(ConfigSelectorComponent::new(
        resolved,
        settings,
        cwd,
        agent_dir,
        move || {
            on_close.borrow_mut().get_or_insert(true);
        },
        move || {
            on_exit.borrow_mut().get_or_insert(false);
        },
    )));
    let closed = run_standalone(selector, tui, outcome, |_| false).unwrap_or(true);
    cortexcode_code_tui_theme::stop_theme_watcher();
    closed
}

pub fn resume_picker(theme_name: Option<&str>, session_dir: PathBuf) -> Option<PathBuf> {
    cortexcode_code_tui_theme::init_theme(theme_name, true);
    let selected = select_session(
        current_sessions_loader(session_dir),
        all_sessions_loader(),
        None,
    );
    cortexcode_code_tui_theme::stop_theme_watcher();
    selected
}
