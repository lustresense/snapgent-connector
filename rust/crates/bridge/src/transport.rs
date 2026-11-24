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
    task::JoinSet,
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
            json!({"type":"authenticated","nonce":server_nonce})
                .to_string()
                .into(),
        ))
        .await?;
    Ok(())
}
async fn client(mut socket: WebSocket, state: Arc<State>) {
    if authenticate(&mut socket, &state).await.is_err() {
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
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _=shutdown.changed()=>break,
            message=reader.next()=>{match message{
                Some(Ok(Message::Text(text)))=>{match serde_json::from_str::<Value>(&text){
                    Ok(request) if request.is_object()=>{let kind=request["type"].as_str().unwrap_or("");if kind=="call_tool"{let key=request["session"].as_str().filter(|s|snapgent_security::valid_nonce(s)).unwrap_or("legacy").to_owned();if tasks.len()>=32 || (!sessions.contains_key(&key) && sessions.len()>=128){let _=writer.send(Message::Text(json!({"type":"tool_result","id":request["id"],"ok":false,"kind":"busy","error":"Request queue full"}).to_string().into())).await;}else{let session=sessions.entry(key).or_insert_with(||Arc::new(Mutex::new(Session::default()))).clone();let state=state.clone();let tx=tx.clone();tasks.spawn(async move{let mut session=session.lock().await;if let Some(response)=protocol::handle(state,&mut session,request).await{let _=tx.send(response).await;}});}}else if tasks.len()<34{let state=state.clone();let tx=tx.clone();tasks.spawn(async move{if let Some(response)=protocol::handle(state,&mut Session::default(),request).await{let _=tx.send(response).await;}});}else{let _=writer.send(Message::Text(json!({"type":"error","error":"Request queue full"}).to_string().into())).await;}},
                    _=>{let _=writer.send(Message::Text(json!({"type":"error","error":"Invalid JSON request"}).to_string().into())).await;}
                }},
                Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,
                _=>{}
            }},
            Some(response)=rx.recv()=>{if writer.send(Message::Text(response.to_string().into())).await.is_err(){break;}},
            update=catalog.recv()=>{match update{Ok(value)=>{if writer.send(Message::Text(value.to_string().into())).await.is_err(){break;}},Err(broadcast::error::RecvError::Lagged(_))=>{},Err(_)=>break}},
            _=tasks.join_next(),if !tasks.is_empty()=>{}
        }
    }
    tasks.abort_all();
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
    data["version"] = json!("3.9.14");
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
