//! Derived compact overlay.
//!
//! The overlay is replayed from `MicroCompactEntry` facts in `log.jsonl`. It is
//! NOT a source of truth and is never persisted — it only shapes the current
//! context view, never the audit log. When an entry was compacted more than
//! once, the `MicroCompactEntry` with the largest seq wins.

use crate::session::model::{CompactedEntryView, EntryId, SessionEntry, SessionEntryKind};
use std::collections::HashMap;

pub struct CompactOverlay {
    views: HashMap<EntryId, CompactedEntryView>,
    /// seq of the compact entry that produced each view, for largest-seq-wins.
    seqs: HashMap<EntryId, u64>,
}

impl CompactOverlay {
    pub fn from_entries(entries: &[SessionEntry]) -> Self {
        let mut overlay = CompactOverlay {
            views: HashMap::new(),
            seqs: HashMap::new(),
        };

        for entry in entries {
            let SessionEntryKind::MicroCompact(mc) = &entry.kind else {
                continue;
            };
            let compact_seq = entry.meta.seq;
            for original_id in &mc.compacted_entry_ids {
                let refs: Vec<_> = mc
                    .archived_artifacts
                    .iter()
                    .filter(|a| &a.source_entry_id == original_id)
                    .cloned()
                    .collect();
                let replacement_preview =
                    refs.first().map(|a| a.preview.clone()).unwrap_or_default();

                let keep = overlay
                    .seqs
                    .get(original_id)
                    .map(|prev| compact_seq >= *prev)
                    .unwrap_or(true);
                if !keep {
                    continue;
                }
                overlay.seqs.insert(original_id.clone(), compact_seq);
                overlay.views.insert(
                    original_id.clone(),
                    CompactedEntryView {
                        original_entry_id: original_id.clone(),
                        replacement_preview,
                        artifact_refs: refs,
                        compact_entry_id: entry.meta.id.clone(),
                    },
                );
            }
        }

        overlay
    }

    pub fn view_for(&self, entry_id: &EntryId) -> Option<&CompactedEntryView> {
        self.views.get(entry_id)
    }
}
