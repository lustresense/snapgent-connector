#![forbid(unsafe_code)]
mod approval;
mod events;
mod manager;
mod protocol;
mod transport;
use anyhow::{Context, Result};
pub use approval::{Approval, Approvals};
pub use events::EventHub;
pub use manager::Manager;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{broadcast, watch};

pub struct State {
    pub manager: Manager,
    pub events: EventHub,
    pub approvals: Approvals,
    token: String,
    origins: Vec<String>,
    catalog: broadcast::Sender<Value>,
    shutdown: watch::Sender<bool>,
}
impl State {
    pub fn new(root: PathBuf, token: String, origins: Vec<String>) -> Result<Arc<Self>> {
        anyhow::ensure!(
            snapgent_security::valid_nonce(&token),
            "Invalid bridge credential"
        );
        let (catalog, _) = broadcast::channel(32);
        let (shutdown, _) = watch::channel(false);
        Ok(Arc::new(Self {
            manager: Manager::new(root),
            events: EventHub::default(),
            approvals: Approvals::default(),
            token,
            origins,
            catalog,
            shutdown,
        }))
    }
    pub fn publish_catalog(&self, value: Value) {
        let _ = self.catalog.send(value);
    }
    pub fn request_stop(&self) {
        self.approvals.deny_all();
        self.shutdown.send_replace(true);
    }
    pub async fn run(self: Arc<Self>, port: u16, stream_port: u16) -> Result<()> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .with_context(|| format!("Cannot bind 127.0.0.1:{port}: another bridge may be running"))?;
        let stream =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, stream_port))
                .await
                .with_context(|| format!("Cannot bind 127.0.0.1:{stream_port}: another bridge may be running"))?;
        let optional_ipv6 = tokio::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port))
            .await
            .ok();
        let mut tasks = tokio::task::JoinSet::<Result<()>>::new();
        let state = self.clone();
        tasks.spawn(async move { transport::serve_ws(listener, state).await });
        if let Some(listener) = optional_ipv6 {
            let state = self.clone();
            tasks.spawn(async move { transport::serve_ws(listener, state).await });
        }
        let state = self.clone();
        tasks.spawn(async move { transport::serve_http(stream, state).await });
        self.events.log(&format!(
            "Bridge listening on localhost:{port}; events on {stream_port}"
        ));
        let state = self.clone();
        tasks.spawn(async move{
            state.manager.start().await?;
            let mut previous=Value::Null;let mut shutdown=state.shutdown.subscribe();
            loop{if *shutdown.borrow(){break;}let snapshot=state.manager.snapshot().await;if previous!=snapshot{state.publish_catalog(snapshot.clone());previous=snapshot;}
                tokio::select!{_=shutdown.changed()=>break,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
            }Ok(())
        });
        let mut stop = self.shutdown.subscribe();
        let outcome = if *stop.borrow() {
            Ok(())
        } else {
            tokio::select! {_=stop.changed()=>Ok(()),task=tasks.join_next()=>match task{Some(Ok(result))=>result,Some(Err(err))=>Err(err.into()),None=>Ok(())}}
        };
        self.request_stop();
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        self.manager.stop().await;
        self.events.log("Bridge stopped");
        outcome
    }
}
