use crate::{
    State,
    protocol::{self, Session},
};
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{
        Query, State as AxumState,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    response::{
        Html, IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast, mpsc},
    task::{AbortHandle, JoinSet},
};

pub async fn serve_ws(listener: TcpListener, state: Arc<State>) -> Result<()> {
    let app = Router::new().route("/", get(upgrade)).with_state(state);
    axum::serve(listener, app).await?;
    Ok(())
}
async fn upgrade(
    AxumState(state): AxumState<Arc<State>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let origin = match headers.get(header::ORIGIN) {
        Some(v) => match v.to_str() {
            Ok(s) => Some(s),
            Err(_) => return StatusCode::FORBIDDEN.into_response(),
        },
        None => None,
    };
    if !snapgent_security::trusted_origin(origin, &state.origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(8 * 1024 * 1024)
        .max_frame_size(8 * 1024 * 1024)
        .on_upgrade(move |socket| client(socket, state))
}
async fn read_auth(socket: &mut WebSocket) -> Result<Value> {
    let message = tokio::time::timeout(Duration::from_secs(5), socket.recv())
        .await?
        .context("Authentication disconnected")??;
    let Message::Text(text) = message else {
        bail!("Invalid authentication frame")
    };
    if text.len() > 1024 {
        bail!("Invalid authentication frame");
    }
    Ok(serde_json::from_str(&text)?)
}
async fn authenticate(socket: &mut WebSocket, state: &State) -> Result<()> {
    let hello = read_auth(socket).await?;
    let nonce = hello["nonce"]
        .as_str()
        .filter(|s| snapgent_security::valid_nonce(s))
        .context("Invalid nonce")?;
    if hello["type"] != "auth_hello" {
        bail!("Authentication required");
    }
    let server_nonce = snapgent_security::fresh_nonce()?;
    let proof = snapgent_security::proof(&state.token, "server", nonce, &server_nonce)?;
    socket
        .send(Message::Text(
            json!({"type":"auth_challenge","nonce":server_nonce,"proof":proof})
                .to_string()
                .into(),
        ))
        .await?;
    let auth = read_auth(socket).await?;
    let expected = snapgent_security::proof(&state.token, "client", nonce, &server_nonce)?;
    if auth["type"] != "authenticate"
        || !snapgent_security::constant_match(&expected, auth["proof"].as_str().unwrap_or(""))
    {
        bail!("Authentication failed");
    }
    socket
        .send(Message::Text(
            json!({"type":"authenticated","nonce":server_nonce,"ui_owned":true})
                .to_string()
                .into(),
        ))
        .await?;
    Ok(())
}
async fn client(mut socket: WebSocket, state: Arc<State>) {
    let mut closing = state.shutdown.subscribe();
    let authenticated = if *closing.borrow() {
        false
    } else {
        tokio::select! {
            _ = closing.changed() => false,
            result = authenticate(&mut socket, &state) => result.is_ok(),
        }
    };
    if !authenticated {
        let _ = socket.close().await;
        return;
    }
    state.events.log("Authenticated local client connected");
    let mut catalog = state.catalog.subscribe();
    let mut shutdown = state.shutdown.subscribe();
    let initial = state.manager.snapshot().await;
    if socket
        .send(Message::Text(initial.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    let (mut writer, mut reader) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Value>(32);
    let mut tasks = JoinSet::new();
    let mut sessions: HashMap<String, Arc<Mutex<Session>>> = HashMap::new();
    let mut calls: HashMap<(String, String), (AbortHandle, Value)> = HashMap::new();
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _=shutdown.changed()=>break,
            message = reader.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let request = match serde_json::from_str::<Value>(&text) {
                            Ok(v) if v.is_object() => v,
                            _ => {
                                let _ = writer.send(Message::Text(json!({"type":"error","error":"Invalid JSON request"}).to_string().into())).await;
                                continue;
                            }
                        };
                        calls.retain(|_, (task, _)| !task.is_finished());
                        let kind = request["type"].as_str().unwrap_or("");
                        let call_key = request["session"].as_str()
                            .filter(|s| snapgent_security::valid_nonce(s))
                            .zip(request["call_id"].as_str().filter(|s| !s.is_empty() && s.len() <= 128))
                            .map(|(session, call)| (session.to_owned(), call.to_owned()));
                        if kind == "recover_tool" {
                            let response = state.receipts.recover(&request).await;
                            let _ = writer.send(Message::Text(response.to_string().into())).await;
                        } else if kind == "cancel_tool" {
                            let receipt_cancelled = state.receipts.cancel(&request).await;
                            let cancelled = call_key.and_then(|key| calls.remove(&key));
                            let found = receipt_cancelled || cancelled.is_some();
                            if let Some((task, id)) = cancelled {
                                task.abort();
                                let _ = writer.send(Message::Text(json!({"type":"tool_result","id":id,"ok":false,"kind":"cancelled","error":"Cancellation requested; verify partial changes"}).to_string().into())).await;
                            }
                            let _ = writer.send(Message::Text(json!({"type":"tool_result","id":request["id"],"ok":true,"cancelled":found}).to_string().into())).await;
                        } else if kind == "call_tool" {
                            let key = request["session"].as_str().filter(|s| snapgent_security::valid_nonce(s)).unwrap_or("legacy").to_owned();
                            if tasks.len() >= 32 || (!sessions.contains_key(&key) && sessions.len() >= 128) ||
                                call_key.as_ref().is_some_and(|key| calls.contains_key(key)) {
                                let _ = writer.send(Message::Text(json!({"type":"tool_result","id":request["id"],"ok":false,"kind":"busy","error":"Request queue full or duplicate active call"}).to_string().into())).await;
                            } else {
                                let session = sessions.entry(key).or_insert_with(|| Arc::new(Mutex::new(Session::default()))).clone();
                                let state = state.clone();
                                let tx = tx.clone();
                                if request["server"] == "local" && call_key.is_some() {
                                    // The socket waiter is disposable; the receipt owns execution.
                                    let id = request["id"].clone();
                                    // Register before processing another socket frame (including
                                    // disconnect and cancellation), then spawn only the waiter.
                                    match state.receipts.begin(state.clone(), session, request).await {
                                        Ok(receipt) => {
                                            tasks.spawn(async move {
                                                let response = crate::recovery::Receipts::wait(receipt, id).await;
                                                let _ = tx.send(response).await;
                                            });
                                        }
                                        Err(response) => {
                                            let _ = writer.send(Message::Text(response.to_string().into())).await;
                                        }
                                    }
                                } else {
                                    let id = request["id"].clone();
                                    let task = tasks.spawn(async move {
                                        let mut session = session.lock().await;
                                        if let Some(response) = protocol::handle(state, &mut session, request).await {
                                            let _ = tx.send(response).await;
                                        }
                                    });
                                    if let Some(key) = call_key { calls.insert(key, (task, id)); }
                                }
                            }
                        } else if tasks.len() < 34 {
                            if kind == "workspace_release" {
                                if let Some(key) = request["session"].as_str() {
                                    state.receipts.cancel_session(key).await;
                                    if !calls.keys().any(|(session, _)| session == key) {
                                        sessions.remove(key);
                                    }
                                }
                            }
                            let state = state.clone();
                            let tx = tx.clone();
                            tasks.spawn(async move {
                                if let Some(response) = protocol::handle(state, &mut Session::default(), request).await {
                                    let _ = tx.send(response).await;
                                }
                            });
                        } else {
                            let _ = writer.send(Message::Text(json!({"type":"error","id":request["id"],"error":"Request queue full"}).to_string().into())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            },
            Some(response)=rx.recv()=>{if writer.send(Message::Text(response.to_string().into())).await.is_err(){break;}},
            update=catalog.recv()=>{match update{Ok(value)=>{if writer.send(Message::Text(value.to_string().into())).await.is_err(){break;}},Err(broadcast::error::RecvError::Lagged(_))=>{},Err(_)=>break}},
            _=tasks.join_next(),if !tasks.is_empty()=>{}
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    let _ = tokio::time::timeout(Duration::from_millis(500), writer.send(Message::Close(None))).await;
    state.events.log("Local client disconnected");
}

fn authorize(headers: &HeaderMap, state: &State) -> Option<Response> {
    if !snapgent_security::authorized_http(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
        &state.token,
    ) {
        return Some(
            (
                StatusCode::UNAUTHORIZED,
                [
                    (
                        header::WWW_AUTHENTICATE,
                        "Basic realm=\"Snapgent\", charset=\"UTF-8\"",
                    ),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                "Authentication required",
            )
                .into_response(),
        );
    }
    if headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("cross-site") {
        return Some(StatusCode::FORBIDDEN.into_response());
    }
    None
}
fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (k, v) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
    ] {
        headers.insert(k, axum::http::HeaderValue::from_static(v));
    }
    response
}
pub async fn serve_http(listener: TcpListener, state: Arc<State>) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/status", get(status))
        .route("/events", get(events))
        .route("/stream", get(events))
        .with_state(state);
    axum::serve(listener, app).await?;
    Ok(())
}
async fn index(AxumState(state): AxumState<Arc<State>>, headers: HeaderMap) -> Response {
    if let Some(response) = authorize(&headers, &state) {
        return response;
    }
    secure(Html(include_str!("dashboard.html")).into_response())
}
async fn status(AxumState(state): AxumState<Arc<State>>, headers: HeaderMap) -> Response {
    if let Some(response) = authorize(&headers, &state) {
        return response;
    }
    let mut data = state.events.snapshot();
    data["version"] = json!("3.9.15");
    secure(Json(data).into_response())
}
async fn events(
    AxumState(state): AxumState<Arc<State>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(response) = authorize(&headers, &state) {
        return response;
    }
    let since = query.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
    let (recent, receiver) = state.events.subscribe(since);
    let shutdown = state.shutdown.subscribe();
    let stream = futures_util::stream::unfold(
        (VecDeque::from(recent), receiver, shutdown),
        |(mut recent, mut receiver, mut shutdown)| async move {
            loop {
                if *shutdown.borrow() {
                    return None;
                }
                let frame = if let Some(frame) = recent.pop_front() {
                    frame
                } else {
                    tokio::select! {_=shutdown.changed()=>return None,result=receiver.recv()=>match result{Ok(frame)=>frame,Err(broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>return None}}
                };
                let event = Event::default()
                    .id(frame["seq"].to_string())
                    .event(frame["event"].as_str().unwrap_or("progress"))
                    .data(frame.to_string());
                return Some((Ok::<_, Infallible>(event), (recent, receiver, shutdown)));
            }
        },
    );
    secure(
        Sse::new(stream)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(15))
                    .text("ping"),
            )
            .into_response(),
    )
}
