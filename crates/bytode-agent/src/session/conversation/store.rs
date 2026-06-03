//! Append-only canonical conversation store. Writes to
//! `<session_root>/conversation/log.jsonl`. This is the single source of truth
//! for `ContextBuilder`; `events/log.jsonl` is audit-only.

use crate::error::Result;
use crate::session::conversation::model::CanonicalRecord;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

pub struct CanonicalConversationStore {
    log_path: PathBuf,
    writer: File,
    next_seq: u64,
}

impl CanonicalConversationStore {
    pub fn open(session_root: PathBuf) -> Result<Self> {
        let conv_dir = session_root.join("conversation");
        std::fs::create_dir_all(&conv_dir)?;

        let log_path = conv_dir.join("log.jsonl");

        let (writer, next_seq) = if log_path.exists() {
            let next = replay_from_path(&log_path)?
                .last()
                .map(|r| r.meta.seq + 1)
                .unwrap_or(0);
            let file = OpenOptions::new().append(true).open(&log_path)?;
            (file, next)
        } else {
            (File::create(&log_path)?, 0)
        };

        Ok(CanonicalConversationStore {
            log_path,
            writer,
            next_seq,
        })
    }

    pub fn append(&mut self, record: CanonicalRecord) -> Result<()> {
        let line = serde_json::to_string(&record)?;
        writeln!(self.writer, "{}", line)?;
        self.writer.flush()?;
        self.next_seq = self.next_seq.max(record.meta.seq + 1);
        Ok(())
    }

    pub fn replay(&self) -> Result<Vec<CanonicalRecord>> {
        replay_from_path(&self.log_path)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }
}

fn replay_from_path(path: &PathBuf) -> Result<Vec<CanonicalRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<CanonicalRecord>(&line) {
            Ok(record) => records.push(record),
            Err(e) => {
                return Err(crate::error::BytodeError::Session(format!(
                    "conversation/log.jsonl line {}: corrupt record: {}",
                    line_no + 1,
                    e
                )));
            }
        }
    }

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation::model::{
        CanonicalAssistantPart, CanonicalAssistantResponse, CanonicalContent, CanonicalMessageId,
        CanonicalMeta, CanonicalRecordKind, CanonicalToolResultPart, CanonicalToolResults,
        CanonicalToolStatus, CanonicalTurnFinished, CanonicalTurnStarted, ResponseId, ToolCallId,
        TurnId,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("bytode-cc-{tag}-{}-{nanos}", std::process::id()))
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn append_and_replay_round_trips() {
        let root = temp_root("round-trip");
        let mut store = CanonicalConversationStore::open(root).unwrap();

        store
            .append(CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 0,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                    turn_id: TurnId("t1".into()),
                    user_message_id: CanonicalMessageId("m1".into()),
                    content: "hello".into(),
                }),
            })
            .unwrap();

        let records = store.replay().unwrap();
        assert_eq!(records.len(), 1);
        let CanonicalRecordKind::TurnStarted(ts) = &records[0].kind else {
            panic!("expected TurnStarted");
        };
        assert_eq!(ts.content, "hello");
        assert_eq!(store.next_seq(), 1);
    }

    #[test]
    fn new_session_starts_empty() {
        let root = temp_root("empty");
        let store = CanonicalConversationStore::open(root).unwrap();
        assert_eq!(store.replay().unwrap().len(), 0);
        assert_eq!(store.next_seq(), 0);
    }

    #[test]
    fn replay_resumes_after_reopen() {
        let root = temp_root("resume");

        {
            let mut store = CanonicalConversationStore::open(root.clone()).unwrap();
            store
                .append(CanonicalRecord {
                    meta: CanonicalMeta {
                        seq: 0,
                        created_at: now(),
                    },
                    kind: CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                        turn_id: TurnId("t1".into()),
                    }),
                })
                .unwrap();
        }

        {
            let store = CanonicalConversationStore::open(root.clone()).unwrap();
            let records = store.replay().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(store.next_seq(), 1);
        }
    }

    #[test]
    fn corrupt_line_returns_error() {
        let root = temp_root("corrupt");
        let conv_dir = root.join("conversation");
        std::fs::create_dir_all(&conv_dir).unwrap();
        std::fs::write(conv_dir.join("log.jsonl"), "not valid json\n").unwrap();

        let result = CanonicalConversationStore::open(root);
        assert!(result.is_err());
    }

    #[test]
    fn full_turn_writes_all_record_kinds() {
        let root = temp_root("full-turn");
        let mut store = CanonicalConversationStore::open(root).unwrap();
        let now = now();

        store
            .append(CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 0,
                    created_at: now,
                },
                kind: CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                    turn_id: TurnId("t1".into()),
                    user_message_id: CanonicalMessageId("m1".into()),
                    content: "do it".into(),
                }),
            })
            .unwrap();

        store
            .append(CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 1,
                    created_at: now,
                },
                kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    message_id: CanonicalMessageId("m2".into()),
                    parts: vec![CanonicalAssistantPart::ToolCall {
                        tool_call_id: ToolCallId("call_1".into()),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.rs"}),
                    }],
                }),
            })
            .unwrap();

        store
            .append(CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 2,
                    created_at: now,
                },
                kind: CanonicalRecordKind::ToolResults(CanonicalToolResults {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    results: vec![CanonicalToolResultPart {
                        tool_call_id: ToolCallId("call_1".into()),
                        status: CanonicalToolStatus::Ok,
                        content: CanonicalContent::Inline("file content".into()),
                    }],
                }),
            })
            .unwrap();

        store
            .append(CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 3,
                    created_at: now,
                },
                kind: CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                    turn_id: TurnId("t1".into()),
                }),
            })
            .unwrap();

        let records = store.replay().unwrap();
        assert_eq!(records.len(), 4);
        assert!(matches!(
            records[0].kind,
            CanonicalRecordKind::TurnStarted(_)
        ));
        assert!(matches!(
            records[1].kind,
            CanonicalRecordKind::AssistantResponse(_)
        ));
        assert!(matches!(
            records[2].kind,
            CanonicalRecordKind::ToolResults(_)
        ));
        assert!(matches!(
            records[3].kind,
            CanonicalRecordKind::TurnFinished(_)
        ));
    }
}
