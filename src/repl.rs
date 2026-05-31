use crate::agent::{Agent, AgentMode, AgentOutput};
use crate::project::ProjectProfile;
use crate::ui::{layout::UiState, statusbar::StatusBarState, Terminal};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use futures::FutureExt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

pub async fn run(
    agent: Agent,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
    chat_history_init: Vec<String>,
) -> Agent {
    let result = std::panic::AssertUnwindSafe(run_inner(
        agent,
        profile,
        git_branch,
        git_dirty,
        detection_source,
        chat_history_init,
    ))
    .catch_unwind()
    .await;
    match result {
        Ok(agent) => agent,
        Err(_) => {
            Terminal::restore().ok();
            eprintln!("bytode panicked, terminal restored");
            std::process::exit(1);
        }
    }
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

    let terminal: Arc<Mutex<Terminal>> = Arc::new(Mutex::new(
        Terminal::init().expect("failed to init terminal"),
    ));
    let mut user_input = String::new();
    let mut sidebar_visible = false;

    let chat_history: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(chat_history_init));
    let scroll_offset: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

    {
        let cancel = agent_opt
            .as_ref()
            .expect("agent missing")
            .cancel_token();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            cancel.store(true, Ordering::Relaxed);
        });
    }

    let mut event_stream = crossterm::event::EventStream::new();
    let mut turn_task: Option<tokio::task::JoinHandle<Agent>> = None;
    let mut streaming = false;
    let mut last_msg: Option<std::time::Instant> = None;

    let primary_lang = format!("{} [{}]", profile.primary, profile.build_system);
    let ds = detection_source.to_string();
    let gb = git_branch;
    let gd = git_dirty;

    loop {
        if let Some(a) = agent_opt.as_ref() {
            let history = chat_history.lock().expect("history lock");
            let offset = *scroll_offset.lock().expect("scroll lock");
            let snapshot = history.clone();
            drop(history);

            let state = UiState {
                sidebar_visible,
                tool_names: a.tool_names(),
                status: StatusBarState {
                    dir: std::env::current_dir()
                        .map(|d| d.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    git_branch: gb.clone(),
                    git_dirty: gd,
                    task: "idle".to_string(),
                    model: a.model_name().to_string(),
                    ctx_used: a.context_used(),
                    ctx_total: a.context_total(),
                    tool_count: a.tool_names().len(),
                    session_cost: a.session_cost(),
                    session_calls: a.session_call_count(),
                    mode: mode_str(a.mode()),
                    elapsed: elapsed_str(last_msg),
                },
                tool_result: None,
                history: snapshot,
                streaming: None,
                scroll_offset: offset,
                user_input: user_input.clone(),
                approval: None,
                primary_language: primary_lang.clone(),
                detection_source: ds.clone(),
            };

            let mut t = terminal.lock().expect("terminal lock");
            let _ = t.draw(|frame| crate::ui::layout::render_ui(frame, &state));
        }

        tokio::select! {
            biased;

            result = wait_turn(&mut turn_task) => {
                turn_task = None;
                match result {
                    Some(a) => {
                        agent_opt = Some(a);
                        streaming = false;
                        *scroll_offset.lock().expect("scroll lock") = 0;
                    }
                    None => {
                        Terminal::restore().ok();
                        return agent_opt.take().unwrap_or_else(|| {
                            eprintln!("fatal: agent lost after task panic");
                            std::process::exit(1);
                        });
                    }
                }
            }

            event = event_stream.next() => {
                let Some(Ok(event)) = event else { continue };

                match event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if streaming {
                                if let Some(ref a) = agent_opt {
                                    a.cancel_token().store(true, Ordering::Relaxed);
                                }
                            } else {
                                Terminal::restore().ok();
                                return agent_opt.take().expect("agent missing on exit");
                            }
                        }
                        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            sidebar_visible = !sidebar_visible;
                        }
                        KeyCode::Up | KeyCode::PageUp => {
                            let mut off = scroll_offset.lock().expect("scroll lock");
                            let step = if key.code == KeyCode::PageUp { 20 } else { 3 };
                            *off = (*off + step).min(10_000);
                        }
                        KeyCode::Down | KeyCode::PageDown => {
                            let mut off = scroll_offset.lock().expect("scroll lock");
                            let step = if key.code == KeyCode::PageDown { 20 } else { 3 };
                            *off = off.saturating_sub(step);
                        }
                        KeyCode::Char(c) => { user_input.push(c); }
                        KeyCode::Backspace => { user_input.pop(); }
                        KeyCode::Enter => {
                            let input = std::mem::take(&mut user_input);
                            if input == "/exit" || input == "/quit" {
                                Terminal::restore().ok();
                                return agent_opt.take().expect("agent missing on exit");
                            }

                            if let Some(ref mut a) = agent_opt {
                                if handle_slash(a, &input, &chat_history) { continue; }
                            }
                            if input.is_empty() || streaming { continue; }

                            {
                                let mut h = chat_history.lock().expect("history lock");
                                h.push(format!("\u{25b8} {input}"));
                            }
                            *scroll_offset.lock().expect("scroll lock") = 0;
                            streaming = true;
                            let start_time = std::time::Instant::now();
                            last_msg = Some(start_time);

                            let mut a = agent_opt.take().expect("agent already taken");
                            let tool_names = a.tool_names();
                            let model_s = a.model_name().to_string();
                            let mode_s = mode_str(a.mode());

                            let terminal_clone = Arc::clone(&terminal);
                            let history_clone = Arc::clone(&chat_history);
                            let history_clone2 = Arc::clone(&chat_history);
                            let scroll_clone = Arc::clone(&scroll_offset);
                            let input_clone = input.clone();
                            let pl = primary_lang.clone();
                            let ds_clone = ds.clone();
                            let gb_clone = gb.clone();
                            let dir_s = std::env::current_dir()
                                .map(|d| d.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let start_time = start_time;

                            let streaming_buf = Arc::new(Mutex::new(String::new()));
                            let buf_clone = Arc::clone(&streaming_buf);
                            let current_tool = Arc::new(Mutex::new(String::new()));
                            let tool_clone = Arc::clone(&current_tool);
                            let last_draw = Arc::new(Mutex::new(std::time::Instant::now()));

                            let handle = tokio::spawn(async move {
                                let result = a.run_turn_streaming(&input_clone, move |chunk| {
                                    buf_clone.lock().expect("buf lock").push_str(chunk);
                                    let trimmed = chunk.trim();
                                    if let Some(rest) = trimmed.strip_prefix("\u{27f3} ") {
                                        if let Some((name, _)) = rest.split_once('(') {
                                            *tool_clone.lock().expect("tool lock") = name.to_string();
                                        }
                                    }
                                    *scroll_clone.lock().expect("scroll lock") = 0;

                                    let now = std::time::Instant::now();
                                    if now.duration_since(*last_draw.lock().expect("last draw lock")).as_millis() < 20 {
                                        return;
                                    }
                                    *last_draw.lock().expect("last draw lock") = now;

                                    if let Ok(mut t) = terminal_clone.lock() {
                                        let h = history_clone.lock().expect("history lock");
                                        let start = h.len().saturating_sub(10);
                                        let snapshot: Vec<String> = h.iter().skip(start).cloned().collect();
                                        let streamed = buf_clone.lock().expect("buf lock").clone();
                                        let tool = tool_clone.lock().expect("tool lock").clone();
                                        let task_label = if tool.is_empty() {
                                            "thinking...".into()
                                        } else {
                                            format!("{tool}()...")
                                        };

                                        let _ = t.draw(|frame| {
                                            let s = UiState {
                                                sidebar_visible,
                                                tool_names: tool_names.clone(),
                                                status: StatusBarState {
                                                    dir: dir_s.clone(),
                                                    git_branch: gb_clone.clone(),
                                                    git_dirty: gd,
                                                    task: task_label,
                                                    model: model_s.clone(),
                                                    ctx_used: 0,
                                                    ctx_total: 1_000_000,
                                                    tool_count: tool_names.len(),
                                                    session_cost: 0.0,
                                                    session_calls: 0,
                                                    mode: mode_s.clone(),
                                                    elapsed: elapsed_str(Some(start_time)),
                                                },
                                                tool_result: None,
                                                history: snapshot,
                                                streaming: if streamed.is_empty() {
                                                    None
                                                } else {
                                                    Some(streamed)
                                                },
                                                scroll_offset: 0,
                                                user_input: String::new(),
                                                approval: None,
                                                primary_language: pl.clone(),
                                                detection_source: ds_clone.clone(),
                                            };
                                            crate::ui::layout::render_ui(frame, &s);
                                        });
                                    }
                                }).await;

                                let final_text = streaming_buf.lock().expect("buf lock").clone();
                                let mut h = history_clone2.lock().expect("history lock");
                                match result {
                                    Ok(AgentOutput::Text(_)) => {
                                        if !final_text.is_empty() { h.push(final_text); }
                                    }
                                    Err(e) => { h.push(format!("Error: {e}")); }
                                }
                                a
                            });

                            turn_task = Some(handle);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
}

fn handle_slash(agent: &mut Agent, input: &str, chat_history: &Arc<Mutex<Vec<String>>>) -> bool {
    if input.starts_with("/model") {
        let parts: Vec<&str> = input.split_whitespace().collect();
        let mut h = chat_history.lock().expect("history lock");
        h.push(format!("\u{25b8} {input}"));
        if parts.len() == 1 {
            h.push(format!("Current model: {}", agent.model_name()));
            h.push("Usage: /model pro | /model flash".to_string());
        } else {
            let model = match parts[1].to_lowercase().as_str() {
                "pro" | "deepseek-v4-pro" => "deepseek-v4-pro",
                "flash" | "deepseek-v4-flash" => "deepseek-v4-flash",
                name => {
                    h.push(format!("Unknown model: {name}. Use pro or flash."));
                    return true;
                }
            };
            agent.set_model(model.to_string());
            h.push(format!("Switched to model: {model}"));
        }
        return true;
    }
    if input.starts_with("/plan") {
        let toggle = input.split_whitespace().nth(1).map(|s| s.to_lowercase());
        let new_mode = match toggle.as_deref() {
            Some("on") | Some("enable") => Some(AgentMode::Plan),
            Some("off") | Some("disable") => Some(AgentMode::Normal),
            None | Some("toggle") => Some(if agent.mode() == AgentMode::Plan {
                AgentMode::Normal
            } else {
                AgentMode::Plan
            }),
            _ => None,
        };
        if let Some(mode) = new_mode {
            agent.set_mode(mode);
            let label = match mode {
                AgentMode::Plan => "plan (read-only)",
                AgentMode::Normal => "build",
            };
            let mut h = chat_history.lock().expect("history lock");
            h.push(format!("\u{25b8} {input}"));
            h.push(format!(
                "Mode: {label} | tools: {}",
                agent.tool_names().join(", ")
            ));
        }
        return true;
    }
    false
}

fn mode_str(mode: AgentMode) -> String {
    match mode {
        AgentMode::Normal => "build".into(),
        AgentMode::Plan => "plan".into(),
    }
}

fn elapsed_str(since: Option<std::time::Instant>) -> String {
    match since {
        Some(t) => {
            let ms = t.elapsed().as_millis();
            if ms < 1000 {
                format!("{}ms", ms)
            } else if ms < 60_000 {
                format!("{:.1}s", ms as f64 / 1000.0)
            } else {
                let secs = ms / 1000;
                format!("{}m{}s", secs / 60, secs % 60)
            }
        }
        None => "0ms".into(),
    }
}

async fn wait_turn(task: &mut Option<tokio::task::JoinHandle<Agent>>) -> Option<Agent> {
    match task.as_mut() {
        Some(handle) => match handle.await {
            Ok(a) => Some(a),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}
