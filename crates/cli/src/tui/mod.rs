//! AX terminal UI.
//!
//! Layout, modeled on Codex's TUI architecture:
//!
//! ```text
//! ┌───────────────────────────────────────────┐
//! │ startup information card (once/session)   │
//! │ transcript (scrolling, no println REPL)   │
//! │                                  ...      │
//! │ slash command popup (above composer)      │
//! │ › composer (gray, fixed, multi-line)      │
//! │ model · mode · session · ctx n%     ready │
//! └───────────────────────────────────────────┘
//! ```
//!
//! The bottom pane owns the composer, the slash popup and a stack of modal
//! views (model/session pickers, approval dialog). Agent turns run on a
//! background task so approval requests can be served by the UI thread.

pub mod bottom_pane;
pub mod catalog_refresh;
pub mod commands;
pub mod markdown;
pub mod startup;
mod terminal_backend;
pub mod theme;
pub mod transcript;

use std::{
    collections::VecDeque,
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    terminal::{disable_raw_mode, enable_raw_mode},
};
use model::{Message, Role};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use runtime_core::{AgentEvent, AgentKernel, AllowAll, ApprovalPolicy};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tool::SafetyLevel;

use bottom_pane::{ApprovalDialog, BottomPane, SlashKeyOutcome, ViewOutcome};
use startup::StartupInfo;
use transcript::{Transcript, TranscriptKind};

use crate::{ModelSelection, PermissionConfig, PermissionDecision, ReplState, run_prompt_with};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const ACTIVE_POLL: Duration = Duration::from_millis(80);
const IDLE_POLL: Duration = Duration::from_secs(60);

/// Messages sent from the worker task to the UI thread.
enum WorkerMessage {
    Event(AgentEvent),
    Approval(ApprovalRequest),
}

/// How the user resolved a tool approval prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApprovalChoice {
    AllowOnce,
    AllowSession,
    Deny,
}

struct ApprovalRequest {
    tool: String,
    input: Value,
    safety: SafetyLevel,
    reply: oneshot::Sender<ApprovalChoice>,
}

/// Approval policy that surfaces requests as modal dialogs in the TUI.
struct ChannelApproval {
    tx: mpsc::UnboundedSender<WorkerMessage>,
    permissions: tokio::sync::Mutex<PermissionConfig>,
}

#[async_trait]
impl ApprovalPolicy for ChannelApproval {
    async fn approve(&self, tool: &str, input: &Value, safety: SafetyLevel) -> bool {
        let decision = {
            let permissions = self.permissions.lock().await;
            permissions.for_tool(tool, input)
        };
        match decision {
            PermissionDecision::Allow => return true,
            PermissionDecision::Deny => return false,
            PermissionDecision::Ask if safety == SafetyLevel::Safe => return true,
            PermissionDecision::Ask => {}
        }
        let (tx, rx) = oneshot::channel();
        let sent = self.tx.send(WorkerMessage::Approval(ApprovalRequest {
            tool: tool.to_owned(),
            input: input.clone(),
            safety,
            reply: tx,
        }));
        if sent.is_err() {
            return false;
        }
        match rx.await.unwrap_or(ApprovalChoice::Deny) {
            ApprovalChoice::AllowOnce => true,
            ApprovalChoice::Deny => false,
            ApprovalChoice::AllowSession => {
                let capability = PermissionConfig::capability_for(tool, input).to_owned();
                self.permissions
                    .lock()
                    .await
                    .set(capability, PermissionDecision::Allow);
                true
            }
        }
    }
}

/// Handle to the running agent turn.
struct ActiveTurn {
    done: oneshot::Receiver<(ReplState, Result<String>)>,
}

struct App {
    transcript: Transcript,
    model: String,
    session: String,
    /// Stable session identifier, printed on exit so it can be resumed later.
    session_id: String,
    directory: String,
    context_percent: usize,
    context_window: usize,
    context_tokens: usize,
    reasoning: Option<String>,
    working: bool,
    /// Animation frame for the working spinner.
    spin: usize,
    /// Inputs queued while a turn is running, drained when the agent frees up.
    pending: VecDeque<String>,
    /// Whether the current turn has started emitting content (hides the
    /// center spinner once text streams in).
    streaming: bool,
}

impl App {
    fn new(selection: &ModelSelection) -> Self {
        let directory = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
            .to_string();
        let mut transcript = Transcript::new();
        transcript.card(StartupInfo::new(
            VERSION,
            &selection.model,
            "New Session",
            &directory,
        ));
        Self {
            transcript,
            model: selection.model.clone(),
            session: "New Session".to_owned(),
            session_id: String::new(),
            directory,
            context_percent: 0,
            context_window: 0,
            context_tokens: 0,
            reasoning: None,
            working: false,
            spin: 0,
            pending: VecDeque::new(),
            streaming: false,
        }
    }

    fn push(&mut self, kind: TranscriptKind, text: impl Into<String>) {
        self.transcript.push(kind, text);
    }

    fn reset_for_session(&mut self, title: &str, selection: &ModelSelection) {
        title.clone_into(&mut self.session);
        self.transcript.clear();
        self.transcript.card(StartupInfo::new(
            VERSION,
            &selection.model,
            title,
            &self.directory,
        ));
    }

    fn sync_metadata(&mut self, state: &ReplState, selection: &ModelSelection) {
        self.model.clone_from(&selection.model);
        self.session = state
            .current_session
            .as_ref()
            .map_or_else(|| "New Session".to_owned(), |session| session.title.clone());
        self.session_id = state
            .current_session
            .as_ref()
            .map_or_else(String::new, |session| session.id.clone());
        let estimated = state.runtime.as_ref().map_or_else(
            || estimate_messages(&state.loaded_messages),
            AgentKernel::estimated_context_tokens,
        );
        self.context_window = selection.context_capacity();
        self.context_tokens = estimated;
        self.reasoning = selection.reasoning_effort.map(|effort| effort.to_string());
        self.context_percent = estimated
            .saturating_mul(100)
            .checked_div(selection.context_capacity())
            .unwrap_or_default()
            .min(100);
    }

    fn sync_status_line(&self, pane: &mut BottomPane) {
        let status = pane.status_mut();
        status.model.clone_from(&self.model);
        status.session.clone_from(&self.session);
        status.working = self.working;
        status.directory.clone_from(&self.directory);
        status.context_percent = self.context_percent;
        status.context_window = self.context_window;
        status.context_tokens = self.context_tokens;
        status.reasoning.clone_from(&self.reasoning);
        status.spin = self.spin;
    }

    fn apply_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::TurnStarted => {
                self.working = true;
                self.streaming = false;
            }
            AgentEvent::ModelStarted { provider, model } => {
                let _ = (provider, model);
            }
            // Reasoning deltas are private model working text. Keep the compact
            // animated `Thinking` status instead of dumping chain-of-thought
            // into the transcript and consuming the whole viewport.
            AgentEvent::ThinkingDelta { delta: _ } | AgentEvent::TurnFinished => {}
            AgentEvent::ContentDelta { delta } => {
                self.streaming = true;
                self.transcript.push_agent_delta(&delta);
            }
            AgentEvent::ToolStarted { name } => {
                self.transcript.tool_started(name);
            }
            AgentEvent::ToolFinished { name, success } => {
                self.transcript.tool_finished(&name, success);
            }
            AgentEvent::ContextCompressed {
                removed_messages, ..
            } => {
                self.push(
                    TranscriptKind::Status,
                    format!("Compressed {removed_messages} older messages"),
                );
            }
        }
    }
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), DisableMouseCapture);
        let _ = disable_raw_mode();
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn run_tui(
    mut selection: ModelSelection,
    data_dir: PathBuf,
    skills_dir: PathBuf,
    mcp_config: Option<PathBuf>,
    allow_dangerous: bool,
) -> Result<()> {
    // Title the terminal tab/window "ax" so Windows Terminal labels this tab
    // (mirrors how pi titles its tab, e.g. "π - hzl"). Printed before raw mode
    // so the OSC escape is processed by the shell.
    print!("\x1b]0;ax\x07");
    let _ = io::stdout().flush();
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    crossterm::execute!(io::stdout(), EnableMouseCapture)?;
    let (_, terminal_height) = crossterm::terminal::size()?;
    // Leave a small real scrollback region above the live viewport. A full-height
    // inline viewport combined with `insert_before` causes previously drawn
    // composers to be committed as terminal history and clips the bottom rows.
    // The live region still reaches the bottom, while completed transcript rows
    // grow the normal console buffer exactly like a conventional CLI.
    let live_height = terminal_height
        .saturating_sub(4)
        .max(8)
        .min(terminal_height);
    let mut terminal = Terminal::with_options(
        terminal_backend::WideCellBackend(CrosstermBackend::new(io::stdout())),
        TerminalOptions {
            viewport: Viewport::Inline(live_height),
        },
    )?;

    let mut state: Option<ReplState> = Some(ReplState::new(data_dir, skills_dir, mcp_config)?);
    let mut app = App::new(&selection);
    let mut pane = BottomPane::new(selection.model.clone());
    app.sync_metadata(state.as_ref().expect("state initialized"), &selection);
    app.sync_status_line(&mut pane);

    let (worker_tx, mut worker_rx) = mpsc::unbounded_channel::<WorkerMessage>();
    let (login_tx, mut login_rx) = mpsc::unbounded_channel::<commands::LoginUpdate>();
    let mut active_turn: Option<ActiveTurn> = None;
    let mut force_redraw = true;

    'outer: loop {
        let (scroll_width, scroll_height) = crossterm::terminal::size()?;
        let visible_rows = live_height
            .min(scroll_height)
            .saturating_sub(pane.composer_height(scroll_width).saturating_add(2))
            .max(1);
        let height_before_updates = app.transcript.full_height(scroll_width);
        // ---- Phase 1: drain every queued key before drawing. ---------------
        // A burst of keystrokes is applied in one pass and rendered once, so
        // typing never waits behind repeated full redraws (the previous loop
        // handled only one key per iteration, so fast typing fell behind while
        // a growing transcript was re-rendered).
        let mut key_handled = false;
        while event::poll(Duration::ZERO)? {
            let key = match event::read()? {
                Event::Mouse(mouse) if !pane.has_view() => {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            app.transcript.scroll_up(3, scroll_width, visible_rows);
                        }
                        MouseEventKind::ScrollDown => app.transcript.scroll_down(3),
                        _ => continue,
                    }
                    key_handled = true;
                    continue;
                }
                Event::Resize(_, _) => {
                    force_redraw = true;
                    continue;
                }
                Event::Key(key) => key,
                _ => continue,
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            key_handled = true;
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break 'outer;
            }

            // Modal views (pickers / approval dialog).
            if pane.has_view()
                && let Some((outcome, action)) = pane.handle_view_key(key)
            {
                if matches!(outcome, ViewOutcome::Continue) {
                    continue;
                }
                if let Some(action) = action {
                    match action {
                        // The approval reply is already sent by the dialog itself;
                        // no ReplState exists while the turn is running, so just
                        // close the dialog instead of touching state.
                        bottom_pane::ModalAction::Approval(_) => {
                            pane.pop_view();
                        }
                        action => {
                            commands::apply_modal_action(
                                action,
                                state.as_mut().expect("state available"),
                                &mut selection,
                                &mut app,
                                &mut pane,
                                &login_tx,
                            )
                            .await?;
                            app.sync_metadata(state.as_ref().expect("state available"), &selection);
                        }
                    }
                }
                continue;
            }

            // Slash command popup.
            if pane.slash().is_open()
                && let Some(result) = pane.slash_mut_handle(key)
            {
                match result {
                    SlashRoute::Handled | SlashRoute::Closed => {}
                    SlashRoute::Selected(command) => {
                        pane.composer_mut().clear();
                        pane.slash_mut().close();
                        accept_slash_command(
                            command,
                            state.as_mut().expect("state available"),
                            &mut selection,
                            &mut app,
                            &mut pane,
                        )
                        .await?;
                        app.sync_metadata(state.as_ref().expect("state available"), &selection);
                        if command.name == "/exit" {
                            break 'outer;
                        }
                    }
                }
                continue;
            }

            match key.code {
                KeyCode::PageUp => {
                    app.transcript.scroll_up(
                        usize::from(visible_rows.saturating_sub(1).max(1)),
                        scroll_width,
                        visible_rows,
                    );
                    continue;
                }
                KeyCode::PageDown => {
                    app.transcript
                        .scroll_down(usize::from(visible_rows.saturating_sub(1).max(1)));
                    continue;
                }
                KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.transcript.scroll_from_bottom = 0;
                    continue;
                }
                _ => {}
            }

            // Normal composer editing. The composer stays editable while a turn
            // runs; a submission is queued and handled once the agent frees up.
            let key_outcome = pane.composer_mut().handle_key(key);
            match key_outcome {
                bottom_pane::ComposerKey::Ignored => {}
                bottom_pane::ComposerKey::Edited => {
                    pane.slash_mut().unsuppress();
                    let composer_text = pane.composer().text().to_owned();
                    pane.slash_mut().sync(&composer_text);
                }
                bottom_pane::ComposerKey::Submit => {
                    let submitted = pane.composer().text().trim().to_owned();
                    pane.composer_mut().clear();
                    pane.slash_mut().sync("");
                    if submitted.is_empty() {
                        continue;
                    }
                    app.transcript.scroll_from_bottom = 0;
                    // While a turn is running, queue the input; it is dispatched
                    // once the agent frees up.
                    if app.working {
                        app.push(TranscriptKind::User, submitted.clone());
                        app.pending.push_back(submitted);
                        app.push(
                            TranscriptKind::Status,
                            "Queued — will run after the current task finishes",
                        );
                        continue;
                    }
                    if submitted.starts_with('/') {
                        let keep_running = commands::execute_slash(
                            &submitted,
                            state.as_mut().expect("state available"),
                            &mut selection,
                            &mut app,
                            &mut pane,
                        )
                        .await?;
                        app.sync_metadata(state.as_ref().expect("state available"), &selection);
                        if !keep_running {
                            break 'outer;
                        }
                    } else {
                        app.push(TranscriptKind::User, submitted.clone());
                        start_turn(
                            &submitted,
                            &mut state,
                            &selection,
                            allow_dangerous,
                            &worker_tx,
                            &mut active_turn,
                            &mut app,
                        );
                    }
                }
            }
        }

        // ---- Phase 2: drain background updates. -----------------------------
        let mut updated = false;

        // Drain background Codex login progress (link/code, success, failure).
        while let Ok(update) = login_rx.try_recv() {
            updated = true;
            match update {
                commands::LoginUpdate::Prompt(text) => {
                    app.push(TranscriptKind::Info, format!("Codex login\n{text}"));
                }
                commands::LoginUpdate::Success => {
                    app.push(
                        TranscriptKind::Status,
                        "Codex login successful — discovering available models",
                    );
                    let refresh_data_dir = state
                        .as_ref()
                        .expect("state available during login")
                        .data_dir
                        .clone();
                    let refresh_codex_auth = selection.codex_auth.clone();
                    tokio::spawn(async move {
                        catalog_refresh::refresh_catalogs(refresh_data_dir, refresh_codex_auth)
                            .await;
                    });
                }
                commands::LoginUpdate::Failed(error) => {
                    app.push(
                        TranscriptKind::Error,
                        format!("Codex login failed: {error}"),
                    );
                }
            }
        }

        // Drain worker messages (streaming events, approval requests).
        while let Ok(message) = worker_rx.try_recv() {
            updated = true;
            match message {
                WorkerMessage::Event(event) => {
                    app.apply_event(event);
                }
                WorkerMessage::Approval(request) => {
                    pane.push_view(Box::new(ApprovalDialog::new(
                        request.tool,
                        request.input,
                        request.safety,
                        request.reply,
                    )));
                }
            }
        }

        // Collect a finished turn.
        if let Some(mut turn) = active_turn.take() {
            match turn.done.try_recv() {
                Ok((returned_state, result)) => {
                    updated = true;
                    if let Err(error) = result {
                        app.push(TranscriptKind::Error, format!("{error:#}"));
                    }
                    state = Some(returned_state);
                    app.working = false;
                    app.sync_metadata(state.as_ref().expect("state returned"), &selection);
                    // The agent is free: run any input queued while it was busy.
                    if let Some(next) = app.pending.pop_front() {
                        if next.starts_with('/') {
                            let keep = commands::execute_slash(
                                &next,
                                state.as_mut().expect("state returned"),
                                &mut selection,
                                &mut app,
                                &mut pane,
                            )
                            .await?;
                            app.sync_metadata(state.as_ref().expect("state returned"), &selection);
                            if !keep {
                                break 'outer;
                            }
                        } else {
                            start_turn(
                                &next,
                                &mut state,
                                &selection,
                                allow_dangerous,
                                &worker_tx,
                                &mut active_turn,
                                &mut app,
                            );
                        }
                    }
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    active_turn = Some(ActiveTurn { done: turn.done });
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    updated = true;
                    app.push(TranscriptKind::Error, "agent task terminated unexpectedly");
                    app.working = false;
                }
            }
        }

        // ---- Phase 3: redraw only when something changed or the spinner
        // animation is running. Skipping the idle redraw keeps the loop cheap,
        // so a large transcript never throttles input.
        if key_handled || updated || force_redraw || app.working || pane.has_view() {
            if app.transcript.scroll_from_bottom > 0 {
                let added_rows = app
                    .transcript
                    .full_height(scroll_width)
                    .saturating_sub(height_before_updates);
                app.transcript.scroll_from_bottom =
                    app.transcript.scroll_from_bottom.saturating_add(added_rows);
            }
            force_redraw = false;
            app.spin = app.spin.wrapping_add(1);
            app.sync_status_line(&mut pane);
            // Pi's main-screen renderer grows the real terminal buffer. Commit
            // completed overflow before redrawing the live composer viewport so
            // native terminal scrolling retains the entire conversation and the
            // shell output that preceded AX.
            let (screen_width, screen_height) = crossterm::terminal::size()?;
            let stable_bottom = pane.composer_height(screen_width).saturating_add(2);
            let transcript_rows = live_height
                .min(screen_height)
                .saturating_sub(stable_bottom)
                .max(1);
            // Keep the complete active entry: draining it mid-stream loses
            // Markdown delimiter/fence context and splits a single answer.
            // Do not move the viewport while the user is reading older rows.
            let committed = if app.transcript.scroll_from_bottom == 0 {
                app.transcript
                    .drain_overflow(screen_width, transcript_rows, !app.working)
            } else {
                Vec::new()
            };
            if !committed.is_empty() {
                let height = Paragraph::new(committed.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(screen_width.max(1));
                terminal.insert_before(u16::try_from(height).unwrap_or(u16::MAX), |buffer| {
                    Paragraph::new(committed)
                        .wrap(Wrap { trim: false })
                        .render(buffer.area, buffer);
                })?;
            }
            terminal.draw(|frame| render(frame, &app, &pane))?;
        }

        // ---- Phase 4: wait for the next terminal event. ----------------------
        let poll = if app.working || pane.has_view() {
            ACTIVE_POLL
        } else {
            IDLE_POLL
        };
        let _ = event::poll(poll)?;
    }
    // Graceful exit, pi-style (TuiMainScreen.beforeTerminalStop): keep the
    // session content in place and drop only the TUI frame (composer + footer)
    // so the shell prompt lands cleanly below the transcript, instead of
    // wiping the screen. Everything remains reviewable in the scrollback.
    let _ = terminal.draw(|frame| {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1)])
            .split(area);
        app.transcript.render(frame, chunks[0]);
        frame.set_cursor_position(ratatui::layout::Position::new(0, area.height - 1));
    });
    // Leave raw mode so a plain-text identifier can be printed for the user to
    // copy and resume later via `/resume`.
    let _ = disable_raw_mode();
    if !app.session_id.is_empty() {
        println!("Session ID: {}", app.session_id);
    }
    Ok(())
}

enum SlashRoute {
    Handled,
    Closed,
    Selected(&'static commands::SlashCommandDef),
}

/// Routing helpers for the slash popup, kept here to avoid borrow conflicts
/// between the popup and its owning bottom pane.
impl BottomPane {
    fn slash_mut_handle(&mut self, key: KeyEvent) -> Option<SlashRoute> {
        let outcome = self.slash_mut().handle_key(key)?;
        Some(match outcome {
            SlashKeyOutcome::Handled => SlashRoute::Handled,
            SlashKeyOutcome::Closed => SlashRoute::Closed,
            SlashKeyOutcome::Selected(command) => SlashRoute::Selected(command),
        })
    }
}

fn render(frame: &mut Frame<'_>, app: &App, pane: &BottomPane) {
    let area = frame.area();
    let width = area.width;
    let screen_height = area.height;

    let popup_height = pane.popup_height();
    let input_height = if let Some(height) = pane.view_height(width, screen_height) {
        height
    } else {
        pane.composer_height(width)
    };

    // Like Codex's content-sized viewport, place the composer immediately
    // after the visible transcript instead of reserving the whole screen.
    let bottom_height = popup_height.saturating_add(input_height).saturating_add(2);
    let content_height = u16::try_from(if app.transcript.scroll_from_bottom > 0 {
        app.transcript.full_height(width)
    } else {
        app.transcript.height(width)
    })
    .unwrap_or(u16::MAX);
    let transcript_height = content_height
        .saturating_add(u16::from(app.working && !app.streaming))
        .min(screen_height.saturating_sub(bottom_height));
    let area = Rect {
        height: transcript_height
            .saturating_add(bottom_height)
            .min(screen_height),
        ..area
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(transcript_height),
            Constraint::Length(popup_height),
            Constraint::Length(input_height),
            Constraint::Length(2),
        ])
        .split(area);

    app.transcript.render(frame, chunks[0]);
    // Pi-style minimal working feedback: no provider/model sentence, just one
    // bright Thinking label and a spinner immediately above the composer.
    if app.working && !app.streaming && app.transcript.scroll_from_bottom == 0 {
        draw_thinking_indicator(frame, chunks[0], app.spin);
    }
    pane.render_slash_popup(frame, chunks[1]);
    if pane.has_view() {
        pane.render_active_view(frame, chunks[2]);
    } else {
        // The composer stays editable while a turn runs so the user can type
        // ahead; submissions are queued until the agent frees up.
        if let Some(position) = pane.render_composer(frame, chunks[2], true) {
            frame.set_cursor_position(position);
        }
    }
    pane.render_status(frame, chunks[3]);
}

/// Overlay a centered spinner in the transcript area while the model is
/// thinking but has not yet produced any content.
fn draw_thinking_indicator(frame: &mut Frame<'_>, area: Rect, spin: usize) {
    const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    if area.height == 0 || area.width < 12 {
        return;
    }
    let text = format!("Thinking… {}", SPINNER[spin % SPINNER.len()]);
    let banner = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, theme::accent_bold()))),
        banner,
    );
}

#[allow(clippy::too_many_arguments)]
fn start_turn(
    prompt: &str,
    state: &mut Option<ReplState>,
    selection: &ModelSelection,
    allow_dangerous: bool,
    worker_tx: &mpsc::UnboundedSender<WorkerMessage>,
    active_turn: &mut Option<ActiveTurn>,
    app: &mut App,
) {
    let Some(mut owned_state) = state.take() else {
        return;
    };
    let (done_tx, done_rx) = oneshot::channel();
    let tx = worker_tx.clone();
    let active_selection = selection.clone();
    let approval: Arc<dyn ApprovalPolicy> = if allow_dangerous {
        Arc::new(AllowAll)
    } else {
        Arc::new(ChannelApproval {
            tx: tx.clone(),
            permissions: tokio::sync::Mutex::new(owned_state.permissions.clone()),
        })
    };
    let prompt = prompt.to_owned();
    tokio::spawn(async move {
        let result = run_prompt_with(
            &mut owned_state,
            &active_selection,
            approval,
            &prompt,
            move |event| {
                let _ = tx.send(WorkerMessage::Event(event));
            },
        )
        .await;
        let _ = done_tx.send((owned_state, result));
    });
    *active_turn = Some(ActiveTurn { done: done_rx });
    app.working = true;
    app.streaming = false;
}

async fn accept_slash_command(
    command: &'static commands::SlashCommandDef,
    state: &mut ReplState,
    selection: &mut ModelSelection,
    app: &mut App,
    pane: &mut BottomPane,
) -> Result<()> {
    match command.presentation {
        commands::SlashPresentation::Picker
        | commands::SlashPresentation::Manager
        | commands::SlashPresentation::InfoPanel
        | commands::SlashPresentation::DirectAction => {
            commands::execute_slash(command.name, state, selection, app, pane).await?;
        }
    }
    Ok(())
}

fn restore_transcript(app: &mut App, messages: &[Message], selection: &ModelSelection) {
    app.transcript.clear();
    app.transcript.card(StartupInfo::new(
        VERSION,
        &selection.model,
        &app.session,
        &app.directory,
    ));
    for message in messages {
        match message.role {
            Role::User => app.push(TranscriptKind::User, message.content.clone()),
            Role::Assistant if !message.content.is_empty() => {
                app.push(TranscriptKind::Agent, message.content.clone());
            }
            Role::Tool => app.push(TranscriptKind::Tool, "tool result"),
            Role::System | Role::Assistant => {}
        }
    }
}

fn estimate_messages(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| message.content.chars().count().div_ceil(4) + 4)
        .sum()
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn selection() -> ModelSelection {
        ModelSelection {
            provider: crate::ProviderKind::Deepseek,
            provider_id: "deepseek".to_owned(),
            endpoint: None,
            model: "deepseek-chat".to_owned(),
            codex_auth: None,
            context_window: None,
            reasoning_effort: None,
            supports_tools: true,
        }
    }

    #[test]
    fn renders_card_transcript_composer_and_status() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let app = App::new(&selection());
        let pane = BottomPane::new("deepseek-chat");
        terminal.draw(|frame| render(frame, &app, &pane)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("AX Runtime"));
        assert!(rendered.contains("deepseek-chat"));
        assert!(rendered.contains("›"));
        assert!(rendered.contains("(auto)"));
    }

    #[test]
    fn composer_follows_content_before_and_after_history_commit() {
        for (width, height) in [(40, 12), (100, 30)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut app = App::new(&selection());
            let pane = BottomPane::new("deepseek-chat");
            app.transcript.clear();
            for committed in [false, true] {
                if committed {
                    app.push(TranscriptKind::Agent, "long answer\n\n".repeat(80));
                    assert!(!app.transcript.drain_overflow(width, 4, true).is_empty());
                }
                app.push(TranscriptKind::Agent, "ANSWER_END");
                terminal.draw(|frame| render(frame, &app, &pane)).unwrap();
                let buffer = terminal.backend().buffer();
                let rows: Vec<String> = (0..height)
                    .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
                    .collect();
                let answer = rows
                    .iter()
                    .rposition(|row| row.contains("ANSWER_END"))
                    .unwrap();
                let composer = rows
                    .iter()
                    .position(|row| row.contains("Message AX"))
                    .unwrap();
                assert_eq!(composer - answer, 2, "{}", rows.join("\n"));
            }
        }
    }

    #[test]
    fn completing_long_response_preserves_visible_tail_and_composer_position() {
        for (width, height) in [(40, 12), (100, 30)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut app = App::new(&selection());
            let pane = BottomPane::new("deepseek-chat");
            app.transcript.clear();
            app.working = true;
            app.streaming = true;
            app.push(
                TranscriptKind::Agent,
                format!(
                    "{}\n\nANSWER_END",
                    "**中文回答** with wrapping words and more text.\n\n".repeat(80)
                ),
            );
            terminal.draw(|frame| render(frame, &app, &pane)).unwrap();
            let before = terminal.backend().buffer().clone();
            app.working = false;
            let visible = height - pane.composer_height(width) - 2;
            assert!(
                !app.transcript
                    .drain_overflow(width, visible, true)
                    .is_empty()
            );
            terminal.draw(|frame| render(frame, &app, &pane)).unwrap();
            assert_eq!(terminal.backend().buffer(), &before);
            // Repeated idle frames must not drain the retained tail further.
            assert!(
                app.transcript
                    .drain_overflow(width, visible, true)
                    .is_empty()
            );
        }
    }

    #[test]
    fn streaming_overflow_keeps_markdown_context_until_completion() {
        let mut app = App::new(&selection());
        app.transcript.clear();
        app.apply_event(AgentEvent::TurnStarted);
        app.apply_event(AgentEvent::ContentDelta {
            delta: format!("{}**直接丢个任务", "paragraph\n\n".repeat(30)),
        });
        let committed = app.transcript.drain_overflow(40, 6, !app.working);
        assert!(committed.is_empty());
        app.apply_event(AgentEvent::ContentDelta {
            delta: "\n给我**。\n\n```rust\nlet x = 1;\n```".to_owned(),
        });
        app.working = false;
        let committed = app.transcript.drain_overflow(40, 6, !app.working);
        let mut all_lines = committed;
        for entry in &app.transcript.entries {
            if let transcript::TranscriptEntry::Rendered(lines) = entry {
                all_lines.extend(lines.clone());
            }
        }
        let text = all_lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains("直接丢个任务 给我"));
        assert!(!text.contains("**"));
        assert!(!text.contains("```"));
        assert!(app.transcript.height(40) >= 6);
    }

    #[tokio::test]
    async fn permission_change_refreshes_parent_without_reopening() {
        let mut selection = selection();
        let mut app = App::new(&selection);
        let mut pane = BottomPane::new("deepseek-chat");
        let data = std::env::temp_dir().join(format!("ax-permission-ui-{}", std::process::id()));
        let mut state = ReplState::new(data.clone(), data.join("skills"), None).unwrap();
        let (tx, _) = mpsc::unbounded_channel();
        commands::execute_slash(
            "/permissions",
            &mut state,
            &mut selection,
            &mut app,
            &mut pane,
        )
        .await
        .unwrap();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        // Keep a non-first capability selected through the nested picker.
        pane.handle_view_key(key(KeyCode::Down));
        let (_, action) = pane.handle_view_key(key(KeyCode::Enter)).unwrap();
        commands::apply_modal_action(
            action.unwrap(),
            &mut state,
            &mut selection,
            &mut app,
            &mut pane,
            &tx,
        )
        .await
        .unwrap();
        pane.handle_view_key(key(KeyCode::Down));
        pane.handle_view_key(key(KeyCode::Down));
        let (_, action) = pane.handle_view_key(key(KeyCode::Enter)).unwrap();
        commands::apply_modal_action(
            action.unwrap(),
            &mut state,
            &mut selection,
            &mut app,
            &mut pane,
            &tx,
        )
        .await
        .unwrap();
        assert_eq!(
            state.permissions.get("filesystem-write"),
            crate::PermissionDecision::Deny
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| pane.render_active_view(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = (0..24)
            .map(|y| {
                (0..100)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .find(|row| row.contains("Filesystem Write"))
            .unwrap();
        assert!(row.contains("Deny"), "{row}");
        let (_, action) = pane.handle_view_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            action,
            Some(bottom_pane::ModalAction::SurfaceSelected {
                surface: "permissions".into(),
                id: "filesystem-write".into()
            })
        );
    }

    #[test]
    fn slash_opens_popup_above_composer() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let app = App::new(&selection());
        let mut pane = BottomPane::new("deepseek-chat");
        pane.composer_mut().set("/mo");
        pane.slash_mut().unsuppress();
        let composer_text = pane.composer().text().to_owned();
        pane.slash_mut().sync(&composer_text);
        assert!(pane.slash().is_open());
        terminal.draw(|frame| render(frame, &app, &pane)).unwrap();
        assert!(pane.popup_height() > 0);
    }
}
