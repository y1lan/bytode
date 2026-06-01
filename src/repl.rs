use crate::agent::Agent;
use crate::project::ProjectProfile;
use crate::ui::app_shell::AppShell;
use crate::ui::events::{Effect, KeyAction, TurnId};
use crate::ui::panels::HistoryEntry;
use crate::ui::Terminal;
use crossterm::event::Event;
use futures::{FutureExt, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

pub async fn run(
    agent: Agent,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
    chat: Vec<HistoryEntry>,
) -> Agent {
    std::panic::AssertUnwindSafe(run_inner(
        agent,
        profile,
        git_branch,
        git_dirty,
        detection_source,
        chat,
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Terminal::restore().ok();
        eprintln!("bytode panicked, terminal restored");
        std::process::exit(1);
    })
}

struct StreamEvent {
    turn_id: TurnId,
    chunk: String,
}

async fn run_inner(
    agent: Agent,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
    chat_history_init: Vec<HistoryEntry>,
) -> Agent {
    let terminal = Arc::new(Mutex::new(Terminal::init().expect("terminal")));
    let mut shell = AppShell::new(
        profile,
        git_branch,
        git_dirty,
        detection_source,
        chat_history_init,
    );
    let mut agent_opt = Some(agent);
    let mut events = crossterm::event::EventStream::new();
    let mut rx: Option<mpsc::UnboundedReceiver<StreamEvent>> = None;
    let mut turn_handle: Option<tokio::task::JoinHandle<(TurnId, Agent, Option<String>)>> = None;
    let mut active_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>> = None;
    let mut last_msg: Option<std::time::Instant> = None;

    loop {
        if let Some(agent) = agent_opt.as_ref() {
            shell.sync_runtime_from_agent(agent, elapsed_fmt(last_msg), spinner_char());
        }

        if let Ok(mut terminal) = terminal.lock() {
            let _ = terminal.draw(|frame| shell.render(frame));
        }

        tokio::select! {
            biased;
            msg = recv_stream(&mut rx) => {
                if let Some(event) = msg {
                    shell.append_stream_chunk(event.turn_id, &event.chunk);
                } else {
                    rx = None;
                }
            }
            result = join_turn(&mut turn_handle) => {
                turn_handle = None;
                active_cancel = None;
                last_msg = None;

                match result {
                    Some((turn_id, agent, maybe_error)) => {
                        agent.reset_cancelled();
                        agent_opt = Some(agent);
                        flush_stream_events(&mut shell, &mut rx);
                        rx = None;
                        shell.finish_turn(turn_id);
                        if let Some(error) = maybe_error {
                            shell.record_runtime_error(error);
                        }
                        if let Some(pending) = shell.take_pending_input() {
                            start_turn(
                                shell.allocate_turn_id(),
                                pending,
                                &mut shell,
                                &mut agent_opt,
                                &mut rx,
                                &mut turn_handle,
                                &mut active_cancel,
                                &mut last_msg,
                            );
                        }
                    }
                    None => {
                        shell.record_runtime_error("Turn failed (task panicked)".into());
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            event = events.next() => {
                let Some(Ok(Event::Key(key_event))) = event else {
                    continue;
                };
                let Some(action) = KeyAction::from_key_event(key_event) else {
                    continue;
                };
                let effects = shell.dispatch_key(action);
                for effect in effects {
                    shell.apply_effect(&effect);
                    match effect {
                        Effect::Exit => {
                            Terminal::restore().ok();
                            if let Some(agent) = agent_opt.take() {
                                return agent;
                            }
                            std::process::exit(0);
                        }
                        Effect::StartTurn { turn_id, input } => {
                            start_turn(
                                turn_id,
                                input,
                                &mut shell,
                                &mut agent_opt,
                                &mut rx,
                                &mut turn_handle,
                                &mut active_cancel,
                                &mut last_msg,
                            );
                        }
                        Effect::CancelTurn(turn_id) => {
                            if shell.active_turn_id() == Some(turn_id) {
                                if let Some(cancel) = active_cancel.as_ref() {
                                    cancel.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                        Effect::HandleSlashCommand(command) => {
                            if let Some(agent) = agent_opt.as_mut() {
                                shell.apply_slash_command(&command, agent);
                            }
                        }
                        Effect::ApproveTool(_)
                        | Effect::RejectTool(_)
                        | Effect::SwitchContentView(_)
                        | Effect::OpenOverlay(_)
                        | Effect::CloseOverlay(_)
                        | Effect::SaveSession
                        | Effect::RestoreTerminal
                        | Effect::ShowNotice(_) => {}
                    }
                }
            }
        }
    }
}

fn flush_stream_events(
    shell: &mut AppShell,
    rx: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
) {
    let Some(receiver) = rx.as_mut() else {
        return;
    };

    loop {
        match receiver.try_recv() {
            Ok(event) => shell.append_stream_chunk(event.turn_id, &event.chunk),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        }
    }
}

fn start_turn(
    turn_id: TurnId,
    input: String,
    shell: &mut AppShell,
    agent_opt: &mut Option<Agent>,
    rx: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    turn_handle: &mut Option<tokio::task::JoinHandle<(TurnId, Agent, Option<String>)>>,
    active_cancel: &mut Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    last_msg: &mut Option<std::time::Instant>,
) {
    let Some(mut agent) = agent_opt.take() else {
        shell.record_runtime_error("Agent unavailable".into());
        return;
    };

    agent.reset_cancelled();
    shell.start_turn(turn_id, &input);
    *last_msg = Some(std::time::Instant::now());

    let cancel = agent.cancel_token();
    cancel.store(false, Ordering::Relaxed);
    *active_cancel = Some(cancel);

    let (tx, new_rx) = mpsc::unbounded_channel();
    *rx = Some(new_rx);

    let handle = tokio::spawn(async move {
        let tx_stream = tx.clone();
        let result = agent
            .run_turn_streaming(&input, move |chunk| {
                let _ = tx_stream.send(StreamEvent {
                    turn_id,
                    chunk: chunk.to_string(),
                });
            })
            .await;
        drop(tx);
        let error = result.err().map(|error| format!("Error: {error}"));
        (turn_id, agent, error)
    });
    *turn_handle = Some(handle);
}

fn elapsed_fmt(since: Option<std::time::Instant>) -> String {
    match since {
        Some(started_at) => {
            let ms = started_at.elapsed().as_millis();
            if ms < 1000 {
                format!("{ms}ms")
            } else if ms < 60_000 {
                format!("{:.1}s", ms as f64 / 1000.0)
            } else {
                format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
            }
        }
        None => "0ms".into(),
    }
}

fn spinner_char() -> char {
    const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    SPINNER[(ms / 100) as usize % SPINNER.len()]
}

async fn recv_stream(rx: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>) -> Option<StreamEvent> {
    match rx.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn join_turn(
    handle: &mut Option<tokio::task::JoinHandle<(TurnId, Agent, Option<String>)>>,
) -> Option<(TurnId, Agent, Option<String>)> {
    match handle.as_mut() {
        Some(handle) => handle.await.ok(),
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{BuildSystem, Language, ProjectProfile};

    fn profile() -> ProjectProfile {
        ProjectProfile {
            primary: Language::Rust,
            build_system: BuildSystem::Cargo,
            all_languages: vec![Language::Rust],
            test_framework: None,
            root: std::path::PathBuf::from("."),
            source_dirs: vec![std::path::PathBuf::from("src")],
            is_workspace: false,
            workspace_members: Vec::new(),
        }
    }

    #[test]
    fn flush_stream_events_drains_buffered_chunks() {
        let mut shell = AppShell::new(&profile(), None, false, "auto", Vec::new());
        shell.start_turn(3, "hello");

        let (tx, receiver) = mpsc::unbounded_channel();
        tx.send(StreamEvent {
            turn_id: 3,
            chunk: "first".into(),
        })
        .unwrap();
        tx.send(StreamEvent {
            turn_id: 3,
            chunk: " second".into(),
        })
        .unwrap();
        drop(tx);

        let mut rx = Some(receiver);
        flush_stream_events(&mut shell, &mut rx);
        shell.finish_turn(3);

        let rendered = shell.rendered_content_text_for_test(4, 80);
        assert!(rendered.iter().any(|line| line.contains("first second")));
    }
}
