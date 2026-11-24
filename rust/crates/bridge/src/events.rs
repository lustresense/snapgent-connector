use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
struct Inner {
    seq: u64,
    recent: VecDeque<Value>,
    busy: BTreeMap<u64, Value>,
    next_call: u64,
}
#[derive(Clone)]
pub struct EventHub {
    inner: Arc<Mutex<Inner>>,
    sender: broadcast::Sender<Value>,
}
impl Default for EventHub {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                seq: 0,
                recent: VecDeque::new(),
                busy: BTreeMap::new(),
                next_call: 0,
            })),
            sender,
        }
    }
}
impl EventHub {
    fn publish_locked(&self, inner: &mut Inner, event: &str, payload: Value) -> Value {
        inner.seq += 1;
        let mut frame = payload.as_object().cloned().unwrap_or_default();
        frame.insert("seq".into(), json!(inner.seq));
        frame.insert("at".into(), json!(now()));
        frame.insert("event".into(), json!(event));
        let frame = Value::Object(frame);
        inner.recent.push_back(frame.clone());
        while inner.recent.len() > 200 {
            inner.recent.pop_front();
        }
        let _ = self.sender.send(frame.clone());
        frame
    }
    pub fn publish(&self, event: &str, payload: Value) -> Value {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        self.publish_locked(&mut inner, event, payload)
    }
    pub fn log(&self, message: &str) {
        self.publish("log", json!({"message":message}));
    }
    pub fn snapshot(&self) -> Value {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let busy = inner
            .busy
            .values()
            .map(|v| {
                let mut v = v.clone();
                v["elapsed_ms"] = json!(
                    ((now() - v["started"].as_f64().unwrap_or_else(now)) * 1000.0).max(0.0) as u64
                );
                v
            })
            .collect::<Vec<_>>();
        json!({"seq":inner.seq,"queue":busy.len(),"busy":busy,"subscribers":self.sender.receiver_count()})
    }
    pub fn recent(&self, since: u64) -> Vec<Value> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recent
            .iter()
            .filter(|v| v["seq"].as_u64().unwrap_or(0) > since)
            .cloned()
            .collect()
    }
    pub fn subscribe(&self, since: u64) -> (Vec<Value>, broadcast::Receiver<Value>) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let receiver = self.sender.subscribe();
        let recent = inner
            .recent
            .iter()
            .filter(|v| v["seq"].as_u64().unwrap_or(0) > since)
            .cloned()
            .collect();
        (recent, receiver)
    }
    pub fn begin(&self, id: Value, tool: &str, server: &str) -> CallGuard {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.next_call += 1;
        let key = inner.next_call;
        let entry = json!({"id":id,"tool":tool,"server":server,"started":now()});
        inner.busy.insert(key, entry.clone());
        self.publish_locked(&mut inner, "tool_start", entry);
        CallGuard {
            hub: self.clone(),
            key,
            finished: false,
        }
    }
    fn end(&self, key: u64, ok: bool, chars: usize) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = inner.busy.remove(&key) {
            let ms =
                ((now() - entry["started"].as_f64().unwrap_or_else(now)) * 1000.0).max(0.0) as u64;
            self.publish_locked(&mut inner,"tool_end",json!({"id":entry["id"],"tool":entry["tool"],"ok":ok,"ms":ms,"chars":chars,"tok":chars/4}));
        }
    }
}
pub struct CallGuard {
    hub: EventHub,
    key: u64,
    finished: bool,
}
impl CallGuard {
    pub fn finish(mut self, ok: bool, chars: usize) {
        self.hub.end(self.key, ok, chars);
        self.finished = true;
    }
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.hub.end(self.key, false, 0);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_cleanup_and_replay() {
        let hub = EventHub::default();
        let a = hub.begin(json!(1), "read_file", "zed");
        let b = hub.begin(json!(1), "read_file", "vscode");
        assert_eq!(hub.snapshot()["queue"], 2);
        a.finish(true, 20);
        drop(b);
        assert_eq!(hub.snapshot()["queue"], 0);
        assert_eq!(hub.recent(0).len(), 4);
    }
}
