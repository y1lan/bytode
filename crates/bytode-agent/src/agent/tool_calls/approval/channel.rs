use super::{ApprovalDecision, ApprovalRequest};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

#[async_trait]
pub trait ApprovalChannel: Send + Sync {
    async fn request_approval(&self, request: ApprovalRequest) -> ApprovalDecision;
}

#[derive(Debug, Clone)]
pub struct ApprovalEvent {
    pub request: ApprovalRequest,
}

pub struct InteractiveApprovalChannel {
    event_tx: mpsc::UnboundedSender<ApprovalEvent>,
    pending: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
}

impl InteractiveApprovalChannel {
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<ApprovalEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let channel = Arc::new(Self {
            event_tx,
            pending: Mutex::new(HashMap::new()),
        });
        (channel, event_rx)
    }

    pub fn approve(&self, request_id: &str) -> bool {
        self.resolve(request_id, ApprovalDecision::Approved)
    }

    pub fn reject(&self, request_id: &str, reason: String) -> bool {
        self.resolve(request_id, ApprovalDecision::Rejected { reason })
    }

    fn resolve(&self, request_id: &str, decision: ApprovalDecision) -> bool {
        let Some(sender) = self
            .pending
            .lock()
            .expect("approval pending mutex")
            .remove(request_id)
        else {
            return false;
        };
        sender.send(decision).is_ok()
    }
}

#[async_trait]
impl ApprovalChannel for InteractiveApprovalChannel {
    async fn request_approval(&self, request: ApprovalRequest) -> ApprovalDecision {
        let (decision_tx, decision_rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("approval pending mutex")
            .insert(request.id.clone(), decision_tx);

        if self
            .event_tx
            .send(ApprovalEvent {
                request: request.clone(),
            })
            .is_err()
        {
            let _ = self.reject(&request.id, "approval UI unavailable".into());
        }

        match decision_rx.await {
            Ok(decision) => decision,
            Err(_) => ApprovalDecision::Rejected {
                reason: "approval channel closed".into(),
            },
        }
    }
}
