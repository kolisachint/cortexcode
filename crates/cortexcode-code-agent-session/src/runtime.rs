//! `core/agent-session-runtime.ts` and `core/session-cwd.ts`: the owner of
//! the current [`AgentSession`] and its cwd-bound services, which replaces
//! the session for `/new`, `/resume`, `/fork`, `/cd` and `/import`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use cortexcode_agent_types::AgentMessage;
use cortexcode_code_session::{default_session_dir, FileEntry, NewSessionOptions, SessionManager};

use crate::hooks::{
    ForkPosition, SessionEvent, SessionShutdownReason, SessionStartEvent, SessionStartReason,
    SessionSwitchReason,
};
use crate::services::{AgentSessionRuntimeDiagnostic, AgentSessionServices};
use crate::session::AgentSession;
use crate::stats::extract_user_message_text;

// ----------------------------------------------------------------------
// session-cwd.ts
// ----------------------------------------------------------------------

/// `SessionCwdIssue`: a stored session whose working directory is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCwdIssue {
    pub session_file: Option<String>,
    pub session_cwd: String,
    pub fallback_cwd: String,
}

/// `getMissingSessionCwdIssue`.
pub fn get_missing_session_cwd_issue(
    session_manager: &SessionManager,
    fallback_cwd: &Path,
) -> Option<SessionCwdIssue> {
    let session_file = session_manager.session_file()?;
    let session_cwd = session_manager.cwd();
    if session_cwd.is_empty() || Path::new(session_cwd).exists() {
        return None;
    }
    Some(SessionCwdIssue {
        session_file: Some(session_file.to_string_lossy().into_owned()),
        session_cwd: session_cwd.to_string(),
        fallback_cwd: fallback_cwd.to_string_lossy().into_owned(),
    })
}

/// `formatMissingSessionCwdPrompt`.
pub fn format_missing_session_cwd_prompt(issue: &SessionCwdIssue) -> String {
    format!(
        "cwd from session file does not exist\n{}\n\ncontinue in current cwd\n{}",
        issue.session_cwd, issue.fallback_cwd
    )
}

/// `formatMissingSessionCwdError`.
fn format_missing_session_cwd_error(issue: &SessionCwdIssue) -> String {
    let session_file = issue
        .session_file
        .as_ref()
        .map(|f| format!("\nSession file: {f}"))
        .unwrap_or_default();
    format!(
        "Stored session working directory does not exist: {}{session_file}\nCurrent working directory: {}",
        issue.session_cwd, issue.fallback_cwd
    )
}

/// `assertSessionCwdExists`.
pub fn assert_session_cwd_exists(
    session_manager: &SessionManager,
    fallback_cwd: &Path,
) -> Result<(), RuntimeError> {
    match get_missing_session_cwd_issue(session_manager, fallback_cwd) {
        Some(issue) => Err(RuntimeError::MissingSessionCwd(issue)),
        None => Ok(()),
    }
}

// ----------------------------------------------------------------------
// Runtime
// ----------------------------------------------------------------------

/// Errors from session replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// `ChangeDirectoryError`: `/cd` target missing or not a directory.
    ChangeDirectory { message: String, path: PathBuf },
    /// `SessionImportFileNotFoundError`.
    ImportFileNotFound(PathBuf),
    /// `MissingSessionCwdError`.
    MissingSessionCwd(SessionCwdIssue),
    /// Any other failure (the TS `Error(message)`).
    Other(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::ChangeDirectory { message, .. } => f.write_str(message),
            RuntimeError::ImportFileNotFound(path) => {
                write!(f, "File not found: {}", path.display())
            }
            RuntimeError::MissingSessionCwd(issue) => {
                f.write_str(&format_missing_session_cwd_error(issue))
            }
            RuntimeError::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RuntimeError {}

fn other(message: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Other(message.to_string())
}

/// What the runtime factory is asked to build.
pub struct RuntimeRequest {
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub session_manager: SessionManager,
    pub session_start_event: Option<SessionStartEvent>,
}

/// `CreateAgentSessionRuntimeResult`.
pub struct CreatedRuntime {
    pub session: AgentSession,
    pub services: AgentSessionServices,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    pub model_fallback_message: Option<String>,
}

/// A running runtime creation.
pub type RuntimeFuture = Pin<Box<dyn Future<Output = Result<CreatedRuntime, String>> + Send>>;

/// `CreateAgentSessionRuntimeFactory`: recreates the cwd-bound services for
/// the request's cwd and creates the session on them.
pub type RuntimeFactory = Arc<dyn Fn(RuntimeRequest) -> RuntimeFuture + Send + Sync>;

/// Result of a replacement that extensions may cancel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplaceResult {
    pub cancelled: bool,
}

/// Result of [`AgentSessionRuntime::fork`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkResult {
    pub cancelled: bool,
    /// The forked-from user message's text, for the editor.
    pub selected_text: Option<String>,
}

/// Result of [`AgentSessionRuntime::change_directory`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeDirectoryResult {
    pub cancelled: bool,
    pub cwd: PathBuf,
}

/// `newSession` options.
#[derive(Default)]
pub struct NewSessionRequest {
    pub parent_session: Option<String>,
    /// Seeds the new session before it is bound (its context is reloaded).
    pub setup: Option<SessionSetup>,
}

/// Seeds a new session's manager (`newSession({ setup })`).
pub type SessionSetup = Box<dyn FnOnce(&mut SessionManager) + Send>;
type RebindSession = Box<dyn FnMut(&AgentSession) + Send>;
type BeforeSessionInvalidate = Box<dyn FnMut() + Send>;

/// `AgentSessionRuntime`. Replacement methods tear the current session down
/// first, then create and apply the next one; a creation error propagates.
pub struct AgentSessionRuntime {
    session: AgentSession,
    services: AgentSessionServices,
    create_runtime: RuntimeFactory,
    diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    model_fallback_message: Option<String>,
    rebind_session: Option<RebindSession>,
    before_session_invalidate: Option<BeforeSessionInvalidate>,
}

fn path_string(path: Option<PathBuf>) -> Option<String> {
    path.map(|p| p.to_string_lossy().into_owned())
}

/// `resolve()`: absolute and lexically normalized, symlinks kept.
fn resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// An in-memory manager's empty session dir means "the default one".
fn session_dir_option(dir: &Path) -> Option<PathBuf> {
    (!dir.as_os_str().is_empty()).then(|| dir.to_path_buf())
}

fn start_event(
    reason: SessionStartReason,
    previous_session_file: Option<String>,
) -> Option<SessionStartEvent> {
    Some(SessionStartEvent {
        reason,
        previous_session_file,
    })
}

impl AgentSessionRuntime {
    pub fn new(created: CreatedRuntime, create_runtime: RuntimeFactory) -> Self {
        Self {
            session: created.session,
            services: created.services,
            create_runtime,
            diagnostics: created.diagnostics,
            model_fallback_message: created.model_fallback_message,
            rebind_session: None,
            before_session_invalidate: None,
        }
    }

    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    pub fn cwd(&self) -> &Path {
        &self.services.cwd
    }

    pub fn diagnostics(&self) -> &[AgentSessionRuntimeDiagnostic] {
        &self.diagnostics
    }

    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
    }

    /// Called with each replacement session once it is applied.
    pub fn set_rebind_session(&mut self, rebind: Option<RebindSession>) {
        self.rebind_session = rebind;
    }

    /// Runs after `session_shutdown` handlers and before the old session is
    /// disposed (host UI teardown).
    pub fn set_before_session_invalidate(&mut self, callback: Option<BeforeSessionInvalidate>) {
        self.before_session_invalidate = callback;
    }

    async fn emit_before_switch(
        &self,
        reason: SessionSwitchReason,
        target_session_file: Option<String>,
    ) -> bool {
        let extensions = self.session.extensions();
        if !extensions.has_handlers("session_before_switch") {
            return false;
        }
        extensions
            .emit_session_event(SessionEvent::BeforeSwitch {
                reason,
                target_session_file,
            })
            .await
            .cancel
    }

    async fn emit_before_fork(&self, entry_id: &str, position: ForkPosition) -> bool {
        let extensions = self.session.extensions();
        if !extensions.has_handlers("session_before_fork") {
            return false;
        }
        extensions
            .emit_session_event(SessionEvent::BeforeFork {
                entry_id: entry_id.to_string(),
                position,
            })
            .await
            .cancel
    }

    async fn teardown_current(
        &mut self,
        reason: SessionShutdownReason,
        target_session_file: Option<String>,
    ) {
        emit_session_shutdown(&self.session, reason, target_session_file).await;
        if let Some(callback) = &mut self.before_session_invalidate {
            callback();
        }
        self.session.dispose();
    }

    async fn create_and_apply(&mut self, request: RuntimeRequest) -> Result<(), RuntimeError> {
        let created = (self.create_runtime)(request).await.map_err(other)?;
        self.session = created.session;
        self.services = created.services;
        self.diagnostics = created.diagnostics;
        self.model_fallback_message = created.model_fallback_message;
        Ok(())
    }

    fn finish_session_replacement(&mut self) {
        if let Some(rebind) = &mut self.rebind_session {
            rebind(&self.session);
        }
    }

    /// `/resume`: switch to another session file.
    pub async fn switch_session(
        &mut self,
        session_path: &Path,
        cwd_override: Option<String>,
    ) -> Result<ReplaceResult, RuntimeError> {
        let target = session_path.to_string_lossy().into_owned();
        if self
            .emit_before_switch(SessionSwitchReason::Resume, Some(target))
            .await
        {
            return Ok(ReplaceResult { cancelled: true });
        }
        let previous = path_string(self.session.session_file());
        let manager = SessionManager::open(session_path, None, cwd_override);
        assert_session_cwd_exists(&manager, self.cwd())?;
        let target_file = path_string(manager.session_file().map(Path::to_path_buf));
        self.teardown_current(SessionShutdownReason::Resume, target_file)
            .await;
        self.create_and_apply(RuntimeRequest {
            cwd: PathBuf::from(manager.cwd()),
            agent_dir: self.services.agent_dir.clone(),
            session_manager: manager,
            session_start_event: start_event(SessionStartReason::Resume, previous),
        })
        .await?;
        self.finish_session_replacement();
        Ok(ReplaceResult::default())
    }

    /// `/new`: a fresh session in the same session directory.
    pub async fn new_session(
        &mut self,
        options: NewSessionRequest,
    ) -> Result<ReplaceResult, RuntimeError> {
        if self
            .emit_before_switch(SessionSwitchReason::New, None)
            .await
        {
            return Ok(ReplaceResult { cancelled: true });
        }
        let previous = path_string(self.session.session_file());
        let session_dir = session_dir_option(self.session.session_manager().session_dir());
        let mut manager = SessionManager::create(self.cwd().to_string_lossy(), session_dir);
        if let Some(parent) = options.parent_session {
            manager.new_session(NewSessionOptions {
                parent_session: Some(parent),
                ..Default::default()
            });
        }
        let target_file = path_string(manager.session_file().map(Path::to_path_buf));
        self.teardown_current(SessionShutdownReason::New, target_file)
            .await;
        self.create_and_apply(RuntimeRequest {
            cwd: self.cwd().to_path_buf(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager: manager,
            session_start_event: start_event(SessionStartReason::New, previous),
        })
        .await?;
        if let Some(setup) = options.setup {
            let messages = {
                let mut manager = self.session.session_manager();
                setup(&mut manager);
                manager.build_context().messages
            };
            self.session.agent().set_messages(messages);
        }
        self.finish_session_replacement();
        Ok(ReplaceResult::default())
    }

    /// `/cd`: move the whole runtime to another directory, starting a fresh
    /// session there. A session dir the user pinned (not derived from the
    /// cwd) travels along.
    pub async fn change_directory(
        &mut self,
        target_cwd: &Path,
    ) -> Result<ChangeDirectoryResult, RuntimeError> {
        let resolved = resolve(target_cwd);
        if !resolved.exists() {
            return Err(RuntimeError::ChangeDirectory {
                message: format!("No such directory: {}", resolved.display()),
                path: resolved,
            });
        }
        if !resolved.is_dir() {
            return Err(RuntimeError::ChangeDirectory {
                message: format!("Not a directory: {}", resolved.display()),
                path: resolved,
            });
        }
        if resolve(self.cwd()) == resolved {
            return Ok(ChangeDirectoryResult {
                cancelled: false,
                cwd: self.cwd().to_path_buf(),
            });
        }
        if self
            .emit_before_switch(SessionSwitchReason::New, None)
            .await
        {
            return Ok(ChangeDirectoryResult {
                cancelled: true,
                cwd: self.cwd().to_path_buf(),
            });
        }
        let previous = path_string(self.session.session_file());
        let current_dir = self.session.session_manager().session_dir().to_path_buf();
        let pinned = (current_dir != default_session_dir(&self.cwd().to_string_lossy()))
            .then_some(current_dir)
            .and_then(|dir| session_dir_option(&dir));
        let manager = SessionManager::create(resolved.to_string_lossy(), pinned);
        let target_file = path_string(manager.session_file().map(Path::to_path_buf));
        self.teardown_current(SessionShutdownReason::New, target_file)
            .await;
        self.create_and_apply(RuntimeRequest {
            cwd: resolved.clone(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager: manager,
            session_start_event: start_event(SessionStartReason::New, previous),
        })
        .await?;
        self.finish_session_replacement();
        Ok(ChangeDirectoryResult {
            cancelled: false,
            cwd: resolved,
        })
    }

    /// `/fork`: before a user message (its text returns for the editor) or
    /// at any entry (the branch up to it is duplicated).
    pub async fn fork(
        &mut self,
        entry_id: &str,
        position: ForkPosition,
    ) -> Result<ForkResult, RuntimeError> {
        if self.emit_before_fork(entry_id, position).await {
            return Ok(ForkResult {
                cancelled: true,
                selected_text: None,
            });
        }
        let invalid = || other("Invalid entry ID for forking");
        let (target_leaf_id, selected_text) = {
            let manager = self.session.session_manager();
            let selected = manager.get_entry(entry_id).ok_or_else(invalid)?;
            match position {
                ForkPosition::At => (selected.id().map(str::to_string), None),
                ForkPosition::Before => match selected {
                    FileEntry::Message {
                        parent_id,
                        message: AgentMessage::User(user),
                        ..
                    } => (
                        parent_id.clone(),
                        Some(extract_user_message_text(&user.content)),
                    ),
                    _ => return Err(invalid()),
                },
            }
        };

        let previous = path_string(self.session.session_file());
        let persisted = self.session.session_manager().is_persisted();
        let manager = if persisted {
            let current_file = self
                .session
                .session_file()
                .ok_or_else(|| other("Persisted session is missing a session file"))?;
            let session_dir = self.session.session_manager().session_dir().to_path_buf();
            match &target_leaf_id {
                None => {
                    let mut manager =
                        SessionManager::create(self.cwd().to_string_lossy(), Some(session_dir));
                    manager.new_session(NewSessionOptions {
                        parent_session: Some(current_file.to_string_lossy().into_owned()),
                        ..Default::default()
                    });
                    manager
                }
                Some(leaf) => {
                    let mut source =
                        SessionManager::open(&current_file, Some(session_dir.clone()), None);
                    let forked = source
                        .create_branched_session(leaf.clone())
                        .map_err(other)?
                        .ok_or_else(|| other("Failed to create forked session"))?;
                    SessionManager::open(forked, Some(session_dir), None)
                }
            }
        } else {
            // In memory: the current manager is branched in place and handed on.
            let mut manager = std::mem::replace(
                &mut *self.session.session_manager(),
                SessionManager::in_memory(self.cwd().to_string_lossy()),
            );
            match &target_leaf_id {
                None => {
                    manager.new_session(NewSessionOptions {
                        parent_session: previous.clone(),
                        ..Default::default()
                    });
                }
                Some(leaf) => {
                    manager
                        .create_branched_session(leaf.clone())
                        .map_err(other)?;
                }
            }
            manager
        };

        let cwd = if persisted && target_leaf_id.is_some() {
            PathBuf::from(manager.cwd())
        } else {
            self.cwd().to_path_buf()
        };
        let target_file = path_string(manager.session_file().map(Path::to_path_buf));
        self.teardown_current(SessionShutdownReason::Fork, target_file)
            .await;
        self.create_and_apply(RuntimeRequest {
            cwd,
            agent_dir: self.services.agent_dir.clone(),
            session_manager: manager,
            session_start_event: start_event(SessionStartReason::Fork, previous),
        })
        .await?;
        self.finish_session_replacement();
        Ok(ForkResult {
            cancelled: false,
            selected_text,
        })
    }

    /// `/import`: copy a session JSONL into the session directory and switch
    /// to it.
    pub async fn import_from_jsonl(
        &mut self,
        input_path: &Path,
        cwd_override: Option<String>,
    ) -> Result<ReplaceResult, RuntimeError> {
        let resolved = resolve(input_path);
        if !resolved.exists() {
            return Err(RuntimeError::ImportFileNotFound(resolved));
        }
        let session_dir = self.session.session_manager().session_dir().to_path_buf();
        std::fs::create_dir_all(&session_dir).map_err(other)?;
        let destination = session_dir.join(resolved.file_name().unwrap_or_default());
        if self
            .emit_before_switch(
                SessionSwitchReason::Resume,
                Some(destination.to_string_lossy().into_owned()),
            )
            .await
        {
            return Ok(ReplaceResult { cancelled: true });
        }
        let previous = path_string(self.session.session_file());
        if resolve(&destination) != resolved {
            std::fs::copy(&resolved, &destination).map_err(other)?;
        }
        let manager = SessionManager::open(&destination, Some(session_dir), cwd_override);
        assert_session_cwd_exists(&manager, self.cwd())?;
        let target_file = path_string(manager.session_file().map(Path::to_path_buf));
        self.teardown_current(SessionShutdownReason::Resume, target_file)
            .await;
        self.create_and_apply(RuntimeRequest {
            cwd: PathBuf::from(manager.cwd()),
            agent_dir: self.services.agent_dir.clone(),
            session_manager: manager,
            session_start_event: start_event(SessionStartReason::Resume, previous),
        })
        .await?;
        self.finish_session_replacement();
        Ok(ReplaceResult::default())
    }

    /// Emit `session_shutdown` (quit) and dispose the session.
    pub async fn dispose(&mut self) {
        emit_session_shutdown(&self.session, SessionShutdownReason::Quit, None).await;
        if let Some(callback) = &mut self.before_session_invalidate {
            callback();
        }
        self.session.dispose();
    }
}

/// `emitSessionShutdownEvent`: only when a handler is registered.
async fn emit_session_shutdown(
    session: &AgentSession,
    reason: SessionShutdownReason,
    target_session_file: Option<String>,
) -> bool {
    let extensions = session.extensions();
    if !extensions.has_handlers("session_shutdown") {
        return false;
    }
    extensions
        .emit_session_event(SessionEvent::Shutdown {
            reason,
            target_session_file,
        })
        .await;
    true
}

/// `createAgentSessionRuntime`: the initial runtime; the factory is kept for
/// later replacements.
pub async fn create_agent_session_runtime(
    create_runtime: RuntimeFactory,
    request: RuntimeRequest,
) -> Result<AgentSessionRuntime, RuntimeError> {
    assert_session_cwd_exists(&request.session_manager, &request.cwd)?;
    let created = create_runtime(request).await.map_err(other)?;
    Ok(AgentSessionRuntime::new(created, create_runtime))
}
