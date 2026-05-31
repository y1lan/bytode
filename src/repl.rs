use crate::agent::{Agent, AgentMode};
use crate::project::ProjectProfile;
use crate::ui::layout::{HistoryEntry, ScrollMode, UiState};
use crate::ui::{statusbar::StatusBarState, Terminal};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use futures::{FutureExt, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub async fn run(
    agent: Agent, profile: &ProjectProfile,
    git_branch: Option<String>, git_dirty: bool,
    detection_source: &str, chat: Vec<String>,
) -> Agent {
    std::panic::AssertUnwindSafe(run_inner(agent, profile, git_branch, git_dirty, detection_source, chat))
        .catch_unwind().await.unwrap_or_else(|_| {
            Terminal::restore().ok();
            eprintln!("bytode panicked, terminal restored");
            std::process::exit(1);
        })
}

enum StreamEvent { Chunk(String), Tool(String) }

async fn run_inner(
    agent: Agent, profile: &ProjectProfile,
    git_branch: Option<String>, git_dirty: bool,
    detection_source: &str, chat_history_init: Vec<String>,
) -> Agent {
    let mut agent_opt = Some(agent);

    let terminal = Arc::new(Mutex::new(Terminal::init().expect("terminal")));
    let entries: Vec<HistoryEntry> = chat_history_init.into_iter().map(|s| {
        if s.starts_with('\u{25b8}') {
            HistoryEntry::User(s.trim_start_matches('\u{25b8}').trim().to_string())
        } else {
            HistoryEntry::Assistant(s)
        }
    }).collect();
    let entries = Arc::new(Mutex::new(entries));

    let mut input = String::new();
    let mut pending: Option<String> = None;
    let mut sidebar = false;
    let mut streaming = false;
    let mut scroll = ScrollMode::Auto;
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
        tokio::spawn(async move { tokio::signal::ctrl_c().await.ok(); cancel.store(true, Ordering::Relaxed); });
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
                    task: if streaming { "thinking...".into() } else { String::new() },
                    model: model.clone(), ctx_used: cu, ctx_total: ct,
                    tool_count: tn.len(), session_cost: cost, session_calls: calls,
                    mode: mode.clone(), elapsed: elapsed_fmt(last_msg), spinner: spinner_char(),
                },
                tool_result: None,
                entries: entries.lock().unwrap().clone(),
                streaming: if streaming { Some(show_stream_buf.lock().unwrap().clone()) } else { None },
                scroll,
                user_input: input.clone(),
                pending: pending.clone(),
                approval: None,
                primary_language: pl.clone(),
                detection_source: ds.clone(),
            };
            if let Ok(mut t) = terminal.lock() { let _ = t.draw(|f| crate::ui::layout::render_ui(f, &st)); }
        }

        tokio::select! {
            biased;
            msg = recv_stream(&mut rx) => {
                match msg {
                    Some(StreamEvent::Chunk(text)) => {
                        stream_buf.push_str(&text);
                        *show_stream_buf.lock().unwrap() = stream_buf.clone();
                    }
                    Some(StreamEvent::Tool(_)) => {}
                    None => {
                        rx = None;
                        streaming = false;
                        last_msg = None;
                        let ft = std::mem::take(&mut stream_buf);
                        if !ft.is_empty() {
                            // Split stream_buf into assistant text segments and tool call segments
                            let mut el = entries.lock().unwrap();
                            let mut buf = String::new();
                            let mut in_tool = false;
                            let mut tool_text = String::new();
                            for line in ft.lines() {
                                if line.contains("\u{27f3} ") {
                                    if in_tool && !tool_text.trim().is_empty() {
                                        el.push(HistoryEntry::Tool { name: String::new(), summary: tool_text.trim().to_string() });
                                    } else if !buf.trim().is_empty() {
                                        el.push(HistoryEntry::Assistant(std::mem::take(&mut buf)));
                                    }
                                    in_tool = true;
                                    let name = extract_tool_name(line);
                                    tool_text.clear();
                                    tool_text.push_str(&format!("{}(", name));
                                    tool_text.push('\n');
                                } else if in_tool {
                                    if line.trim().is_empty() {
                                        el.push(HistoryEntry::Tool { name: String::new(), summary: tool_text.trim().to_string() });
                                        in_tool = false;
                                        tool_text.clear();
                                        buf.clear();
                                    } else {
                                        tool_text.push_str(line);
                                        tool_text.push('\n');
                                    }
                                } else {
                                    buf.push_str(line);
                                    buf.push('\n');
                                }
                            }
                            if in_tool && !tool_text.trim().is_empty() {
                                el.push(HistoryEntry::Tool { name: String::new(), summary: tool_text.trim().to_string() });
                            }
                            if !buf.trim().is_empty() {
                                el.push(HistoryEntry::Assistant(buf.trim().to_string()));
                            }
                        }
                        *show_stream_buf.lock().unwrap() = String::new();
                    }
                }
            }
            result = join_turn(&mut turn_handle) => {
                turn_handle = None;
                match result {
                    Some(a) => agent_opt = Some(a),
                    None => {
                        entries.lock().unwrap().push(HistoryEntry::Error("Turn failed (task panicked)".into()));
                        rx = None;
                        streaming = false;
                        *show_stream_buf.lock().unwrap() = String::new();
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            event = events.next() => {
                let Some(Ok(ev)) = event else { continue };
                match ev {
                    Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            if streaming {
                                if let Some(ref a) = agent_opt { a.cancel_token().store(true, Ordering::Relaxed); }
                            } else {
                                Terminal::restore().ok();
                                if let Some(a) = agent_opt.take() { return a; }
                                std::process::exit(0);
                            }
                        }
                        KeyCode::Char('t') if k.modifiers.contains(KeyModifiers::CONTROL) => sidebar = !sidebar,
                        KeyCode::Up | KeyCode::PageUp => {
                            let step = if k.code == KeyCode::PageUp { 20 } else { 5 };
                            match scroll {
                                ScrollMode::Auto => scroll = ScrollMode::Manual(step),
                                ScrollMode::Manual(n) => scroll = ScrollMode::Manual(n.saturating_add(step).min(100_000)),
                            }
                        }
                        KeyCode::Down | KeyCode::PageDown => {
                            let step = if k.code == KeyCode::PageDown { 20 } else { 5 };
                            match scroll {
                                ScrollMode::Auto => {}
                                ScrollMode::Manual(n) => {
                                    let n2 = n.saturating_sub(step);
                                    scroll = if n2 == 0 { ScrollMode::Auto } else { ScrollMode::Manual(n2) };
                                }
                            }
                        }
                        KeyCode::Char(c) => input.push(c),
                        KeyCode::Backspace => { input.pop(); }
                        KeyCode::Enter if k.modifiers.contains(KeyModifiers::SHIFT) => {
                            input.push('\n');
                        }
                        KeyCode::Enter => {
                            let msg = std::mem::take(&mut input);
                            if msg == "/exit" || msg == "/quit" {
                                Terminal::restore().ok();
                                if let Some(a) = agent_opt.take() { return a; }
                                std::process::exit(0);
                            }
                            if let Some(ref mut a) = agent_opt {
                                if handle_slash(a, &msg, &entries) { continue; }
                            }
                            if msg.is_empty() { continue; }
                            if streaming {
                                *show_stream_buf.lock().unwrap() = format!("[pending] {}", msg);
                                pending = Some(msg);
                                continue;
                            }

                            last_msg = Some(std::time::Instant::now());
                            entries.lock().unwrap().push(HistoryEntry::User(msg.clone()));
                            scroll = ScrollMode::Auto;
                            stream_buf.clear();
                            *show_stream_buf.lock().unwrap() = String::new();
                            streaming = true;

                            let mut a = match agent_opt.take() {
                                Some(a) => a,
                                None => {
                                    entries.lock().unwrap().push(HistoryEntry::Error("Agent unavailable".into()));
                                    continue;
                                }
                            };
                            snap_tools = a.tool_names();
                            snap_model = a.model_name().to_string();
                            snap_mode = mode_str(a.mode());

                            let entries2 = Arc::clone(&entries);
                            let (tx, new_rx) = mpsc::unbounded_channel();
                            let tx2 = tx.clone();
                            rx = Some(new_rx);

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
                                    Err(e) => { entries2.lock().unwrap().push(HistoryEntry::Error(format!("Error: {e}"))); }
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

fn handle_slash(agent: &mut Agent, input: &str, entries: &Arc<Mutex<Vec<HistoryEntry>>>) -> bool {
    let mut h = entries.lock().unwrap();
    if input.starts_with("/model") {
        h.push(HistoryEntry::User(input.to_string()));
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.len() == 1 {
            h.push(HistoryEntry::Assistant(format!("Current model: {}", agent.model_name())));
            h.push(HistoryEntry::Assistant("Usage: /model pro | /model flash".to_string()));
        } else {
            let model = match parts[1].to_lowercase().as_str() {
                "pro" | "deepseek-v4-pro" => "deepseek-v4-pro",
                "flash" | "deepseek-v4-flash" => "deepseek-v4-flash",
                name => { h.push(HistoryEntry::Error(format!("Unknown model: {name}"))); return true; }
            };
            agent.set_model(model.to_string());
            h.push(HistoryEntry::Assistant(format!("Switched to model: {model}")));
        }
        return true;
    }
    if input.starts_with("/plan") {
        h.push(HistoryEntry::User(input.to_string()));
        let toggle = input.split_whitespace().nth(1).map(|s| s.to_lowercase());
        let m = match toggle.as_deref() {
            Some("on") | Some("enable") => Some(AgentMode::Plan),
            Some("off") | Some("disable") => Some(AgentMode::Normal),
            None | Some("toggle") => Some(if agent.mode() == AgentMode::Plan { AgentMode::Normal } else { AgentMode::Plan }),
            _ => None,
        };
        if let Some(mode) = m {
            agent.set_mode(mode);
            let label = match mode { AgentMode::Plan => "plan", AgentMode::Normal => "build" };
            h.push(HistoryEntry::Assistant(format!("Mode: {label} | tools: {}", agent.tool_names().join(", "))));
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
            if ms < 1000 { format!("{ms}ms") } else if ms < 60_000 { format!("{:.1}s", ms as f64/1000.0) } else { format!("{}m{}s", ms/60000, (ms%60000)/1000) }
        }
        None => "0ms".into(),
    }
}

fn spinner_char() -> char {
    const S: &[char] = &['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    S[(ms / 100) as usize % S.len()]
}

async fn recv_stream(rx: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>) -> Option<StreamEvent> {
    match rx.as_mut() { Some(r) => r.recv().await, None => std::future::pending().await }
}

async fn join_turn(h: &mut Option<tokio::task::JoinHandle<Agent>>) -> Option<Agent> {
    match h.as_mut() { Some(handle) => match handle.await { Ok(a) => Some(a), Err(_) => None }, None => std::future::pending().await }
}

fn extract_tool_name(line: &str) -> String {
    // "  ⟳ read_file(src/main.rs)" -> "read_file"
    line.split('\u{27f3}')
        .nth(1)
        .unwrap_or("")
        .trim()
        .split('(')
        .next()
        .unwrap_or("?")
        .to_string()
}
