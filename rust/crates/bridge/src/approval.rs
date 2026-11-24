use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;

pub struct Approval {
    pub title: String,
    pub details: Value,
    reply: oneshot::Sender<bool>,
}
impl Approval {
    pub fn respond(self, allowed: bool) {
        let _ = self.reply.send(allowed);
    }
    pub fn expired(&self) -> bool {
        self.reply.is_closed()
    }
}
#[derive(Clone, Default)]
pub struct Approvals {
    queue: Arc<Mutex<VecDeque<Approval>>>,
    trusted_terminal: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
}
impl Approvals {
    pub fn trusted_terminal(&self) -> bool {
        self.trusted_terminal.load(Ordering::Acquire)
    }
    pub fn set_trusted_terminal(&self, value: bool) {
        self.trusted_terminal.store(value && !self.closed.load(Ordering::Acquire), Ordering::Release);
    }
    pub fn take(&self) -> Option<Approval> {
        let mut queue = self.queue.lock().ok()?;
        while let Some(item) = queue.pop_front() {
            if !item.expired() {
                return Some(item);
            }
        }
        None
    }
    pub fn deny_all(&self) {
        self.closed.store(true, Ordering::Release);
        self.set_trusted_terminal(false);
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
    }
    pub async fn ask(&self, title: &str, details: Value, terminal: bool) -> bool {
        if self.closed.load(Ordering::Acquire) { return false; }
        if terminal && self.trusted_terminal() {
            return true;
        }
        let (tx, rx) = oneshot::channel();
        {
            let Ok(mut queue) = self.queue.lock() else {
                return false;
            };
            if self.closed.load(Ordering::Acquire) { return false; }
            queue.retain(|item| !item.expired());
            if queue.len() >= 16 {
                return false;
            }
            queue.push_back(Approval {
                title: title.into(),
                details,
                reply: tx,
            });
        }
        matches!(
            tokio::time::timeout(Duration::from_secs(90), rx).await,
            Ok(Ok(true))
        )
    }
}
