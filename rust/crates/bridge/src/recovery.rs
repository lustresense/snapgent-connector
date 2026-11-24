// In-memory receipt journal. A transport disconnect must not discard an
// already-dispatched local command. Recovery queries NEVER execute commands.
use crate::{State, protocol::{self, Session}};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};
use tokio::sync::{Mutex, watch};

type Key = (String, String);
struct Entry {
    request: Value,
    result: watch::Receiver<Option<Value>>,
    cancel: watch::Sender<bool>,
    created: Instant,
}
#[derive(Default)]
pub struct Receipts {
    entries: Mutex<HashMap<Key, Entry>>,
}
pub fn key(request: &Value) -> Option<Key> {
    let session = request["session"].as_str().filter(|s| snapgent_security::valid_nonce(s))?;
    let call = request["call_id"].as_str().filter(|s| !s.is_empty() && s.len() <= 128)?;
    Some((session.to_owned(), call.to_owned()))
}
fn error(id: &Value, kind: &str, message: &str) -> Value {
    json!({"type":"tool_result","id":id,"ok":false,"kind":kind,"error":message})
}
fn with_id(mut result: Value, id: &Value) -> Value {
    result["id"] = id.clone();
    result
}
impl Receipts {
    pub async fn begin(&self, state: Arc<State>, session: Arc<Mutex<Session>>, request: Value) -> Result<watch::Receiver<Option<Value>>, Value> {
        let id = request["id"].clone();
        let Some(key) = key(&request) else {
            return Err(error(&id,"tool_error","Invalid call receipt"));
        };
        let mut signature = request.clone();
        let retry_unbound = request["retry_unbound"].as_bool() == Some(true);
        let signature_object = signature.as_object_mut().expect("request object");
        signature_object.remove("id");
        signature_object.remove("retry_unbound");
        let receiver = {
            let mut entries = self.entries.lock().await;
            entries.retain(|_, e| e.result.borrow().is_none() || e.created.elapsed() < Duration::from_secs(900));
            if retry_unbound && entries.get(&key).is_some_and(|e| e.request == signature &&
                e.result.borrow().as_ref().is_some_and(|r| r["kind"] == "workspace_unbound")) {
                entries.remove(&key);
            }
            if let Some(entry) = entries.get(&key) {
                if entry.request != signature {
                    return Err(error(&id,"permission_denied","Call ID already belongs to another request"));
                }
                entry.result.clone()
            } else {
                // A bounded journal; evict only finished entries. Missing receipts
                // are reported as unknown, never silently replayed.
                if entries.len() >= 64 {
                    let oldest = entries.iter().filter(|(_,e)| e.result.borrow().is_some())
                        .min_by_key(|(_,e)| e.created).map(|(k,_)| k.clone());
                    if let Some(old) = oldest { entries.remove(&old); }
                }
                if entries.len() >= 64 || entries.values().filter(|e| e.result.borrow().is_none()).count() >= 16 {
                    return Err(error(&id,"busy","Local execution queue full"));
                }
                let (result_tx, result_rx) = watch::channel(None);
                let (cancel_tx, mut cancel_rx) = watch::channel(false);
                entries.insert(key, Entry{request:signature,result:result_rx.clone(),cancel:cancel_tx,created:Instant::now()});
                let mut shutdown = state.shutdown.subscribe();
                tokio::spawn(async move {
                    let result = if *shutdown.borrow() || *cancel_rx.borrow() {
                        error(&request["id"],"cancelled","Local call cancelled before execution")
                    } else {
                        let request_id = request["id"].clone();
                        let execute = async {
                            let mut session = session.lock().await;
                            protocol::handle(state.clone(), &mut session, request).await
                                .unwrap_or_else(|| error(&request_id,"tool_error","Missing tool result"))
                        };
                        tokio::select! {
                            biased;
                            _ = shutdown.changed() => error(&request_id,"cancelled","Bridge stopping; verify partial changes"),
                            _ = cancel_rx.changed() => error(&request_id,"cancelled","Cancellation requested; verify partial changes"),
                            result = execute => result,
                        }
                    };
                    result_tx.send_replace(Some(result));
                });
                result_rx
            }
        };
        Ok(receiver)
    }
    pub async fn wait(mut receiver: watch::Receiver<Option<Value>>, id: Value) -> Value {
        loop {
            if let Some(result) = receiver.borrow().clone() { return with_id(result, &id); }
            if receiver.changed().await.is_err() { return error(&id,"outcome_unknown","Receipt lost; verify changes before retrying"); }
        }
    }
    pub async fn recover(&self, request: &Value) -> Value {
        let id = &request["id"];
        let Some(key) = key(request) else { return error(id,"outcome_unknown","Invalid recovery key"); };
        let entries = self.entries.lock().await;
        match entries.get(&key) {
            Some(entry) => match entry.result.borrow().clone() {
                Some(result) => with_id(result,id),
                None => error(id,"recovering","Original command is still running"),
            },
            None => error(id,"outcome_unknown","Original receipt unavailable; do not repeat without checking changes"),
        }
    }
    pub async fn cancel(&self, request: &Value) -> bool {
        let Some(key) = key(request) else { return false; };
        let entries = self.entries.lock().await;
        if let Some(entry) = entries.get(&key) {
            if entry.result.borrow().is_none() { entry.cancel.send_replace(true); return true; }
        }
        false
    }
    pub async fn cancel_session(&self, session: &str) {
        let entries = self.entries.lock().await;
        for ((sid,_), entry) in entries.iter() {
            if sid == session && entry.result.borrow().is_none() { entry.cancel.send_replace(true); }
        }
    }
}
