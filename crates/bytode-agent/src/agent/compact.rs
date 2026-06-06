use super::{Agent, AgentMode};
use crate::error::Result;
use crate::llm::{self, LlmOutput};
use crate::session::compact::interactive::{
    self, InteractiveCompactOutput, build_compact_system_msg, commit_to_main_store,
    create_compact_store, generate_canonical_evidence_pack,
};
use crate::session::conversation::select_canonical_source_span;
use crate::session::InteractiveCompactEntry;
use std::path::PathBuf;

impl Agent {
    /// Select a source range, generate an evidence pack, and enter
    /// compact session, and enter `InteractiveCompact` mode.
    pub fn start_interactive_compact(&mut self) -> Result<()> {
        let canonical_records = self.runtime.canonical_records()?;
        let source_range = select_canonical_source_span(&canonical_records).ok_or_else(|| {
            crate::error::BytodeError::Session(
                "当前对话尚短，暂无可压缩的历史：压缩只作用于超出最近窗口的旧对话，请先积累更多轮次后再试。"
                    .into(),
            )
        })?;

        let evidence_pack_ref = generate_canonical_evidence_pack(
            &canonical_records,
            &source_range,
            self.runtime.artifact_store(),
            self.runtime.session_id(),
        )?;

        let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let compact_store = create_compact_store(&project_root)?;
        let system_msg = build_compact_system_msg(&canonical_records, &source_range);
        let start_seq = source_range.start_seq;
        let end_seq = source_range.end_seq_exclusive;

        self.compact = Some(crate::session::compact::interactive::InteractiveCompactState {
            source_range,
            evidence_pack_ref,
            compact_store,
            messages: vec![system_msg],
            current_content: String::new(),
        });
        self.mode = AgentMode::InteractiveCompact;

        tracing::info!(start_seq, end_seq, "interactive compact started");
        Ok(())
    }

    /// Send user input to the compact session as a modification request.
    /// Both the user input and the assistant response are appended to the
    /// compact session's own `log.jsonl`.
    pub async fn run_interactive_compact_turn(
        &mut self,
        input: &str,
    ) -> Result<InteractiveCompactOutput> {
        let state = self.compact.as_mut().ok_or_else(|| {
            crate::error::BytodeError::Session("no active interactive compact session".into())
        })?;

        {
            let meta = state.compact_store.next_meta();
            state.compact_store.commit(crate::session::SessionEntry {
                meta,
                kind: crate::session::SessionEntryKind::UserMessage(
                    crate::session::UserMessageEntry {
                        content: input.to_string(),
                    },
                ),
            })?;
        }

        let prompt = if state.current_content.is_empty() {
            "Generate the initial compressed summary of the source range.".to_string()
        } else {
            format!(
                "Modification request:\n{input}\n\nRespond with the updated compressed content."
            )
        };
        state.messages.push(llm::build_user_message(&prompt));

        let response = self.llm.chat(state.messages.clone(), vec![]).await?;
        let content = match response {
            LlmOutput::Text(t) => t,
            LlmOutput::ToolCalls(calls) => {
                let names = calls
                    .into_iter()
                    .map(|call| call.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(crate::error::BytodeError::Session(format!(
                    "compact session produced unexpected tool call(s): {}",
                    names
                )));
            }
        };

        state.messages.push(llm::build_assistant_text_message(&content));
        state.current_content = content.clone();

        {
            let meta = state.compact_store.next_meta();
            state.compact_store.commit(crate::session::SessionEntry {
                meta,
                kind: crate::session::SessionEntryKind::AssistantMessage(
                    crate::session::AssistantMessageEntry {
                        content: content.clone(),
                    },
                ),
            })?;
        }

        Ok(interactive::InteractiveCompactOutput { content })
    }

    /// Commit the latest compact session content to the main session log.
    pub fn commit_interactive_compact(&mut self) -> Result<InteractiveCompactEntry> {
        let state = self.compact.take().ok_or_else(|| {
            crate::error::BytodeError::Session("no active interactive compact session".into())
        })?;

        if state.current_content.is_empty() {
            return Err(crate::error::BytodeError::Session(
                "cannot commit: compact session has no content yet".into(),
            ));
        }

        let ic_entry = commit_to_main_store(&mut self.runtime.store_mut(), &state)?;
        self.runtime.append_canonical_compact_replacement(
            state.source_range.clone(),
            state.current_content.clone(),
            state.evidence_pack_ref.clone(),
        )?;

        self.mode = AgentMode::Normal;
        tracing::info!(
            start_seq = ic_entry.source_range.start_seq,
            end_seq = ic_entry.source_range.end_seq_exclusive,
            "interactive compact committed"
        );
        Ok(ic_entry)
    }

    /// Abort the interactive compact session without changing the main session.
    pub fn abort_interactive_compact(&mut self) -> Result<()> {
        if self.compact.is_none() {
            return Err(crate::error::BytodeError::Session(
                "no active interactive compact session".into(),
            ));
        }
        self.compact = None;
        self.mode = AgentMode::Normal;
        tracing::info!("interactive compact aborted");
        Ok(())
    }

    pub fn is_in_interactive_compact(&self) -> bool {
        self.mode == AgentMode::InteractiveCompact
    }
}
