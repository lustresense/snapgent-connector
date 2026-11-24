use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use snapgent_bridge::State;
use std::time::Duration;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

#[tokio::test]
async fn authenticated_wire_contract() {
    let root = std::env::temp_dir().join(format!("snapgent-wire-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("config.json"),
        r#"{"mcpServers":{"zed":{},"vscode":{}}}"#,
    )
    .unwrap();
    let token = "ab".repeat(32);
    let origin = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let state = State::new(root.clone(), token.clone(), vec![origin.into()]).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let http = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let runner = tokio::spawn(state.clone().run(port, http));
    let url = format!("ws://127.0.0.1:{port}/");
    let mut ready = false;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ready);
    let mut bad = url.as_str().into_client_request().unwrap();
    bad.headers_mut()
        .insert("origin", "https://example.com".parse().unwrap());
    assert!(connect_async(bad).await.is_err());
    let mut request = url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", origin.parse().unwrap());
    let (mut ws, _) = connect_async(request).await.unwrap();
    let nonce = "12".repeat(32);
    ws.send(Message::Text(
        json!({"type":"auth_hello","nonce":nonce})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let challenge: Value =
        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(challenge["type"], "auth_challenge");
    let server = challenge["nonce"].as_str().unwrap();
    assert_eq!(
        challenge["proof"],
        snapgent_security::proof(&token, "server", &nonce, server).unwrap()
    );
    ws.send(Message::Text(json!({"type":"authenticate","proof":snapgent_security::proof(&token,"client",&nonce,server).unwrap()}).to_string().into())).await.unwrap();
    let authenticated: Value =
        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(authenticated["type"], "authenticated");
    assert_eq!(authenticated["ui_owned"], true);
    let connected: Value =
        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(connected["type"], "connected");
    ws.send(Message::Text(
        json!({"type":"ping","id":123}).to_string().into(),
    ))
    .await
    .unwrap();
    let pong = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let msg = ws.next().await.unwrap().unwrap();
            if let Ok(text) = msg.to_text() {
                let v: Value = serde_json::from_str(text).unwrap();
                if v["type"] == "pong" {
                    break v;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(pong["id"], 123);
    ws.send(Message::Text(json!({"type":"call_tool","id":124,"name":"zed/read_file","server":"vscode","arguments":{"path":"test.txt"}}).to_string().into())).await.unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let msg = ws.next().await.unwrap().unwrap();
            if let Ok(text) = msg.to_text() {
                let v: Value = serde_json::from_str(text).unwrap();
                if v["id"] == 124 {
                    break v;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["kind"], "unknown_tool");
    // Stop while the extension is still connected. The UI shutdown must close
    // the active connection, not only the listening port.
    state.request_stop();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = ws.next().await {
            if message.is_close() { break; }
        }
    }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
