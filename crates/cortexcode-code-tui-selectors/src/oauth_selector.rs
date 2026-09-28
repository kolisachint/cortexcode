//! The provider pickers of `/login` and `/logout`, hoocode
//! `components/oauth-selector.ts`, and `isApiKeyLoginProvider` from
//! `login-controller.ts`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use cortexcode_code_auth::provider_display_names::built_in_provider_display_name;
use cortexcode_code_auth::{AuthCredential, AuthSource, AuthStatus, AuthStorage};
use cortexcode_code_tui_theme::{select_gutter, style_input, theme, SELECT_CURSOR};
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_code_tui_widgets::selected_row_list::{SelectableRow, SelectedRowList};
use cortexcode_tui_components::{Input, Spacer, TruncatedText};
use cortexcode_tui_fuzzy::fuzzy_filter;
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{Component, ComponentHandle};

/// Whether a provider signs in with an API key rather than a subscription
/// (`isApiKeyLoginProvider`). `built_in_provider_ids` defaults to every
/// catalog provider.
pub fn is_api_key_login_provider(
    provider_id: &str,
    oauth_provider_ids: &HashSet<String>,
    built_in_provider_ids: Option<&HashSet<String>>,
) -> bool {
    if built_in_provider_display_name(provider_id).is_some() {
        return true;
    }
    let is_built_in = match built_in_provider_ids {
        Some(ids) => ids.contains(provider_id),
        None => cortexcode_ai_models::get_providers().contains(&provider_id),
    };
    if is_built_in {
        return false;
    }
    !oauth_provider_ids.contains(provider_id)
}

/// How a provider signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    OAuth,
    ApiKey,
}

/// `AuthSelectorProvider`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSelectorProvider {
    pub id: String,
    pub name: String,
    pub auth_type: AuthType,
}

/// Which picker this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMode {
    Login,
    Logout,
}

/// What the picker asks its owner to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthSelectorEvent {
    Select(String),
    Cancel,
}

/// A provider's auth status (`getAuthStatus`).
pub type AuthStatusFn = Box<dyn Fn(&str) -> AuthStatus>;

struct State {
    all: Vec<AuthSelectorProvider>,
    filtered: Vec<AuthSelectorProvider>,
    selected: usize,
    mode: LoginMode,
    auth: Arc<AuthStorage>,
    status: AuthStatusFn,
}

impl State {
    /// `formatStatusIndicator`.
    fn status_indicator(&self, provider: &AuthSelectorProvider) -> String {
        let t = theme();
        let stored = self.auth.get(&provider.id).map(|c| match c {
            AuthCredential::OAuth(_) => AuthType::OAuth,
            AuthCredential::ApiKey { .. } => AuthType::ApiKey,
        });
        if stored == Some(provider.auth_type) {
            return t.fg("success", " ✓ configured");
        }
        if let Some(stored) = stored {
            let label = match stored {
                AuthType::OAuth => "subscription configured",
                AuthType::ApiKey => "API key configured",
            };
            return t.fg("muted", " • ") + &t.fg("warning", label);
        }
        if provider.auth_type != AuthType::ApiKey {
            return t.fg("muted", " • unconfigured");
        }
        let status = (self.status)(&provider.id);
        match status.source {
            Some(AuthSource::Environment) => t.fg(
                "success",
                &format!(" ✓ env: {}", status.label.as_deref().unwrap_or("API key")),
            ),
            Some(AuthSource::Runtime) => t.fg("success", " ✓ runtime API key"),
            Some(AuthSource::Fallback) => t.fg("success", " ✓ custom API key"),
            Some(AuthSource::ModelsJsonKey) => t.fg("success", " ✓ key in models.json"),
            Some(AuthSource::ModelsJsonCommand) => t.fg("success", " ✓ command in models.json"),
            _ => t.fg("muted", " • unconfigured"),
        }
    }

    fn filter(&mut self, query: &str) {
        self.filtered = if query.is_empty() {
            self.all.clone()
        } else {
            fuzzy_filter(&self.all, query, |p| {
                let kind = match p.auth_type {
                    AuthType::OAuth => "oauth",
                    AuthType::ApiKey => "api_key",
                };
                format!("{} {} {kind}", p.name, p.id)
            })
        };
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }
}

/// The list under the query line (`updateList`).
struct ProviderList(Rc<RefCell<State>>);

impl Component for ProviderList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let t = theme();
        let state = self.0.borrow();
        const MAX_VISIBLE: usize = 8;
        let len = state.filtered.len();
        let start = (state.selected as isize - (MAX_VISIBLE / 2) as isize)
            .min(len as isize - MAX_VISIBLE as isize)
            .max(0) as usize;
        let end = (start + MAX_VISIBLE).min(len);
        let rows = (start..end)
            .map(|i| {
                let provider = &state.filtered[i];
                let selected = i == state.selected;
                let prefix = if selected {
                    t.fg("accent", SELECT_CURSOR)
                } else {
                    select_gutter()
                };
                let name = t.fg(if selected { "accent" } else { "text" }, &provider.name);
                SelectableRow {
                    text: format!("{prefix}{name}{}", state.status_indicator(provider)),
                    selected,
                }
            })
            .collect();
        let mut lines = SelectedRowList::new(rows, 1).render(width);
        if start > 0 || end < len {
            let info = t.fg("muted", &format!("  ({}/{len})", state.selected + 1));
            lines.extend(TruncatedText::new(info, 0, 0).render(width));
        }
        if len == 0 {
            let message = if !state.all.is_empty() {
                "No matching providers"
            } else if state.mode == LoginMode::Login {
                "No providers available"
            } else {
                "No providers logged in. Use /login first."
            };
            lines.extend(
                TruncatedText::new(t.fg("muted", &format!("  {message}")), 0, 0).render(width),
            );
        }
        lines
    }
}

/// `OAuthSelectorComponent`: a searchable provider list with each
/// provider's auth state.
pub struct OAuthSelectorComponent {
    frame: InputFrame,
    search: Rc<RefCell<Input>>,
    state: Rc<RefCell<State>>,
    events: Vec<OAuthSelectorEvent>,
    submitted: Rc<RefCell<bool>>,
}

impl OAuthSelectorComponent {
    /// `get_auth_status` defaults to the storage's own status.
    pub fn new(
        mode: LoginMode,
        auth: Arc<AuthStorage>,
        providers: Vec<AuthSelectorProvider>,
        get_auth_status: Option<AuthStatusFn>,
    ) -> Self {
        let status = get_auth_status.unwrap_or_else(|| {
            let auth = auth.clone();
            Box::new(move |id: &str| auth.get_auth_status(id))
        });
        let state = Rc::new(RefCell::new(State {
            filtered: providers.clone(),
            all: providers,
            selected: 0,
            mode,
            auth,
            status,
        }));
        let mut frame = InputFrame::new(InputFrameOptions {
            title: Some(
                match mode {
                    LoginMode::Login => "login provider",
                    LoginMode::Logout => "logout provider",
                }
                .to_string(),
            ),
            ..Default::default()
        });
        let mut input = Input::new();
        style_input(&mut input);
        let submitted: Rc<RefCell<bool>> = Rc::default();
        let flag = submitted.clone();
        input.on_submit = Some(Box::new(move |_: &str| *flag.borrow_mut() = true));
        let search = Rc::new(RefCell::new(input));
        frame.add_child(search.clone());
        frame.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        frame.add_child(Rc::new(RefCell::new(ProviderList(state.clone()))) as ComponentHandle);
        Self {
            frame,
            search,
            state,
            events: Vec::new(),
            submitted,
        }
    }

    fn select_current(&mut self) {
        let state = self.state.borrow();
        if let Some(provider) = state.filtered.get(state.selected) {
            self.events
                .push(OAuthSelectorEvent::Select(provider.id.clone()));
        }
    }

    /// Events since the last call.
    pub fn take_events(&mut self) -> Vec<OAuthSelectorEvent> {
        std::mem::take(&mut self.events)
    }
}

impl Component for OAuthSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "tui.select.up") {
            let mut state = self.state.borrow_mut();
            if !state.filtered.is_empty() {
                state.selected = state.selected.saturating_sub(1);
            }
        } else if kb.matches(data, "tui.select.down") {
            let mut state = self.state.borrow_mut();
            if !state.filtered.is_empty() {
                state.selected = (state.selected + 1).min(state.filtered.len() - 1);
            }
        } else if kb.matches(data, "tui.select.confirm") {
            self.select_current();
        } else if kb.matches(data, "tui.select.cancel") {
            self.events.push(OAuthSelectorEvent::Cancel);
        } else {
            self.search.borrow_mut().handle_input_with(data, &kb);
            if std::mem::take(&mut *self.submitted.borrow_mut()) {
                self.select_current();
            }
            let query = self.search.borrow().get_value().to_string();
            self.state.borrow_mut().filter(&query);
        }
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.search.borrow_mut().set_focused(focused);
    }
}
