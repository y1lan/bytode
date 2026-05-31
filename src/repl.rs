use crate::agent::{Agent, AgentMode, AgentOutput};
use crate::project::ProjectProfile;
use crate::ui::{layout::UiState, statusbar::StatusBarState, Terminal};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use futures::{FutureExt, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub async fn run(
    agent: Agent,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
    chat_history_init: Vec<String>,
) -> Agent {
    std::panic::AssertUnwindSafe(run_inner(
        agent, profile, git_branch, git_dirty, detection_source, chat_history_init,
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Terminal::restore().ok();
        eprintln!("bytode panicked, terminal restored");
        std::process::exit(1);
    })
}

enum StreamEvent {
    Chunk(String),
    Tool(String),
}

async fn run_inner(
    agent: Agent,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
    chat_history_init: Vec<String>,
) -> Agent {
    let mut agent_opt = Some(agent);

    let terminal = Arc::new(Mutex::new(Terminal::init().expect("terminal")));
    let chat: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(chat_history_init));
    let scroll = Arc::new(Mutex::new(0usize));

    let mut input = String::new();
    let mut sidebar = false;
    let mut streaming = false;
    let mut snap_model = String::new();
    let mut snap_mode = String::new();
    let mut snap_tools: Vec<String> = vec![];
    let mut last_msg: Option<std::time::Instant> = None;
    let mut stream_buf = String::new();
    let show_stream_buf = Arc::new(Mutex::new(String::new()));

    let pl = format!("{} [{}]", profile.primary, profile.build_system);
    let ds = detection_source.to_string();
    let gb = git_branch;
    let gd = git_dirty;

    {
        let cancel = agent_opt.as_ref().expect("agent missing").cancel_token();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            cancel.store(true, Ordering::Relaxed);
        });
    }

    let mut events = crossterm::event::EventStream::new();
    let mut rx: Option<mpsc::UnboundedReceiver<StreamEvent>> = None;
    let mut turn_handle: Option<tokio::task::JoinHandle<Agent>> = None;

    loop {
        // Render
        {
            let (tn, model, mode, cu, ct, cost, calls) = if let Some(ref a) = agent_opt {
                (a.tool_names(), a.model_name().to_string(), mode_str(a.mode()),
                 a.context_used(), a.context_total(), a.session_cost(), a.session_call_count())
            } else {
                (snap_tools.clone(), snap_model.clone(), snap_mode.clone(),
                 0u64, 1_000_000u64, 0.0, 0u64)
            };

            let st = UiState {
                sidebar_visible: sidebar,
                tool_names: tn.clone(),
                status: StatusBarState {
                    dir: std::env::current_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
                    git_branch: gb.clone(), git_dirty: gd,
                    task: if streaming { "thinking...".into() } else { "idle".into() },
                    model: model.clone(), ctx_used: cu, ctx_total: ct,
                    tool_count: tn.len(), session_cost: cost, session_calls: calls,
                    mode: mode.clone(), elapsed: elapsed_fmt(last_msg),
                    spinner: spinner_char(),
                },
                tool_result: None,
                history: chat.lock().unwrap().clone(),
                streaming: if streaming { Some(show_stream_buf.lock().unwrap().clone()) } else { None },
                scroll_offset: *scroll.lock().unwrap(),
                user_input: input.clone(),
                approval: None,
                primary_language: pl.clone(),
                detection_source: ds.clone(),
            };
            if let Ok(mut t) = terminal.lock() {
                let _ = t.draw(|frame| crate::ui::layout::render_ui(frame, &st));
            }
        }

        tokio::select! {
            biased;

            msg = recv_stream(&mut rx) => {
                match msg {
                    Some(StreamEvent::Chunk(text)) => {
                        stream_buf.push_str(&text);
                        *show_stream_buf.lock().unwrap() = stream_buf.clone();
                    }
                    Some(StreamEvent::Tool(_name)) => {}
                    None => {
                        rx = None;
                        streaming = false;
                        let ft = std::mem::take(&mut stream_buf);
                        if !ft.is_empty() { chat.lock().unwrap().push(ft); }
                        *show_stream_buf.lock().unwrap() = String::new();
                        *scroll.lock().unwrap() = 0;
                    }
                }
            }

            result = join_turn(&mut turn_handle) => {
                turn_handle = None;
                if let Some(a) = result { agent_opt = Some(a); }
            }

            event = events.next() => {
                let Some(Ok(ev)) = event else { continue };
                match ev {
                    Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            if streaming {
                                if let Some(ref a) = agent_opt {
                                    a.cancel_token().store(true, Ordering::Relaxed);
                                }
                            } else {
                                Terminal::restore().ok();
                                if let Some(a) = agent_opt.take() { return a; }
                                std::process::exit(0);
                            }
                        }
                        KeyCode::Char('t') if k.modifiers.contains(KeyModifiers::CONTROL) => sidebar = !sidebar,
                        KeyCode::Up | KeyCode::PageUp => {
                            let mut off = scroll.lock().unwrap();
                            *off = (*off + if k.code == KeyCode::PageUp { 20 } else { 5 }).min(100_000);
                        }
                        KeyCode::Down | KeyCode::PageDown => {
                            let mut off = scroll.lock().unwrap();
                            *off = off.saturating_sub(if k.code == KeyCode::PageDown { 20 } else { 5 });
                        }
                        KeyCode::Char(c) => input.push(c),
                        KeyCode::Backspace => { input.pop(); }
                        KeyCode::Enter => {
                            let msg = std::mem::take(&mut input);
                            if msg == "/exit" || msg == "/quit" {
                                Terminal::restore().ok();
                                if let Some(a) = agent_opt.take() { return a; }
                                std::process::exit(0);
                            }
                            if let Some(ref mut a) = agent_opt {
                                if handle_slash(a, &msg, &chat) { continue; }
                            }
                            if msg.is_empty() || streaming { continue; }

                            last_msg = Some(std::time::Instant::now());
                            chat.lock().unwrap().push(format!("\u{25b8} {msg}"));
                            *scroll.lock().unwrap() = 0;
                            stream_buf.clear();
                            *show_stream_buf.lock().unwrap() = String::new();
                            streaming = true;

                            let mut a = agent_opt.take().expect("agent already taken");
                            snap_tools = a.tool_names();
                            snap_model = a.model_name().to_string();
                            snap_mode = mode_str(a.mode());

                            let (tx, new_rx) = mpsc::unbounded_channel();
                            let tx2 = tx.clone();
                            rx = Some(new_rx);

                            let chat2 = Arc::clone(&chat);
                            let handle = tokio::spawn(async move {
                                let res = a.run_turn_streaming(&msg, move |chunk| {
                                    let trimmed = chunk.trim();
                                    if let Some(rest) = trimmed.strip_prefix("\u{27f3} ") {
                                        if let Some((name, _)) = rest.split_once('(') {
                                            let _ = tx2.send(StreamEvent::Tool(name.to_string()));
                                        }
                                    }
                                    let _ = tx2.send(StreamEvent::Chunk(chunk.to_string()));
                                }).await;
                                drop(tx);
                                match res {
                                    Err(e) => {
                                        chat2.lock().unwrap().push(format!("Error: {e}"));
                                    }
                                    _ => {}
                                }
                                a
                            });

                            turn_handle = Some(handle);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
}

fn handle_slash(agent: &mut Agent, input: &str, chat: &Arc<Mutex<Vec<String>>>) -> bool {
    let mut h = chat.lock().unwrap();
    if input.starts_with("/model") {
        h.push(format!("\u{25b8} {input}"));
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.len() == 1 {
            h.push(format!("Current model: {}", agent.model_name()));
            h.push("Usage: /model pro | /model flash".to_string());
        } else {
            let model = match parts[1].to_lowercase().as_str() {
                "pro" | "deepseek-v4-pro" => "deepseek-v4-pro",
                "flash" | "deepseek-v4-flash" => "deepseek-v4-flash",
                name => { h.push(format!("Unknown model: {name}")); return true; }
            };
            agent.set_model(model.to_string());
            h.push(format!("Switched to model: {model}"));
        }
        return true;
    }
    if input.starts_with("/plan") {
        h.push(format!("\u{25b8} {input}"));
        let toggle = input.split_whitespace().nth(1).map(|s| s.to_lowercase());
        let new_mode = match toggle.as_deref() {
            Some("on") | Some("enable") => Some(AgentMode::Plan),
            Some("off") | Some("disable") => Some(AgentMode::Normal),
            None | Some("toggle") => Some(if agent.mode() == AgentMode::Plan { AgentMode::Normal } else { AgentMode::Plan }),
            _ => None,
        };
        if let Some(mode) = new_mode {
            agent.set_mode(mode);
            let label = match mode { AgentMode::Plan => "plan", AgentMode::Normal => "build" };
            h.push(format!("Mode: {label} | tools: {}", agent.tool_names().join(", ")));
        }
        return true;
    }
    false
}

fn mode_str(mode: AgentMode) -> String {
    match mode { AgentMode::Normal => "build".into(), AgentMode::Plan => "plan".into() }
}

fn elapsed_fmt(since: Option<std::time::Instant>) -> String {
    match since {
        Some(t) => {
            let ms = t.elapsed().as_millis();
            if ms < 1000 { format!("{ms}ms") }
            else if ms < 60_000 { format!("{:.1}s", ms as f64 / 1000.0) }
            else { format!("{}m{}s", ms / 60000, (ms % 60000) / 1000) }
        }
        None => "0ms".into(),
    }
}

fn spinner_char() -> char {
    const SPIN: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let idx = (now.as_millis() / 100) as usize % SPIN.len();
    SPIN[idx]
}

async fn recv_stream(rx: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>) -> Option<StreamEvent> {
    match rx.as_mut() {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn join_turn(h: &mut Option<tokio::task::JoinHandle<Agent>>) -> Option<Agent> {
    match h.as_mut() {
        Some(handle) => match handle.await { Ok(a) => Some(a), Err(_) => None },
        None => std::future::pending().await,
    }
}
