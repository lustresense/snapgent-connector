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
    let sid = "45".repeat(32);
    state.manager.bind_session(&sid, "local", snapgent_workspace::Workspace::bound(snapgent_workspace::Editor::Local, &root).unwrap()).await.unwrap();
    ws.send(Message::Text(json!({"type":"call_tool","id":125,"name":"run_command","server":"local","session":sid,"call_id":"cancel-test","arguments":{"command":"this-must-not-run"}}).to_string().into())).await.unwrap();
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(approval) = state.approvals.take() { break approval; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    ws.send(Message::Text(json!({"type":"cancel_tool","id":126,"session":sid,"call_id":"cancel-test"}).to_string().into())).await.unwrap();
    let cancelled = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            if let Ok(text) = message.to_text() {
                let result: Value = serde_json::from_str(text).unwrap();
                if result["id"] == 125 { break result; }
            }
        }
    }).await.unwrap();
    assert_eq!(cancelled["kind"], "cancelled");
    tokio::time::timeout(Duration::from_secs(2), async {
        while !approval.expired() { tokio::time::sleep(Duration::from_millis(20)).await; }
    }).await.unwrap();
    // Lose the socket while an original command waits for local approval.
    ws.send(Message::Text(json!({"type":"call_tool","id":127,"name":"run_command","server":"local","session":sid,"call_id":"recover-test","arguments":{"command":"this-must-not-run"}}).to_string().into())).await.unwrap();
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(approval) = state.approvals.take() { break approval; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    ws.close(None).await.unwrap();
    drop(ws);
    let mut request = url.as_str().into_client_request().unwrap();
    request.headers_mut().insert("origin", origin.parse().unwrap());
    let (mut ws, _) = connect_async(request).await.unwrap();
    let nonce = "34".repeat(32);
    ws.send(Message::Text(json!({"type":"auth_hello","nonce":nonce}).to_string().into())).await.unwrap();
    let challenge: Value = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    let server = challenge["nonce"].as_str().unwrap();
    ws.send(Message::Text(json!({"type":"authenticate","proof":snapgent_security::proof(&token,"client",&nonce,server).unwrap()}).to_string().into())).await.unwrap();
    let authenticated: Value = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(authenticated["type"], "authenticated");
    ws.send(Message::Text(json!({"type":"recover_tool","id":128,"session":sid,"call_id":"recover-test"}).to_string().into())).await.unwrap();
    let pending = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            if let Ok(text) = message.to_text() {
                let result: Value = serde_json::from_str(text).unwrap();
                if result["id"] == 128 { break result; }
            }
        }
    }).await.unwrap();
    assert_eq!(pending["kind"],"recovering");
    assert!(!approval.expired());
    approval.respond(false);
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            ws.send(Message::Text(json!({"type":"recover_tool","id":129,"session":sid,"call_id":"recover-test"}).to_string().into())).await.unwrap();
            let result = loop {
                let message = ws.next().await.unwrap().unwrap();
                if let Ok(text) = message.to_text() {
                    let result: Value = serde_json::from_str(text).unwrap();
                    if result["id"] == 129 { break result; }
                }
            };
            if result["kind"] != "recovering" { break result; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    assert_eq!(result["kind"],"permission_denied");
    assert!(state.approvals.take().is_none());
    ws.send(Message::Text(json!({"type":"recover_tool","id":130,"session":sid,"call_id":"missing"}).to_string().into())).await.unwrap();
    let missing = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            if let Ok(text) = message.to_text() {
                let result: Value = serde_json::from_str(text).unwrap();
                if result["id"] == 130 { break result; }
            }
        }
    }).await.unwrap();
    assert_eq!(missing["kind"],"outcome_unknown");
    // A successful write receipt may be fetched/repeated without another write.
    let create = json!({"type":"call_tool","id":131,"name":"create_file","server":"local","session":sid,"call_id":"write-once","arguments":{"path":"once.txt","content":"original"}});
    ws.send(Message::Text(create.to_string().into())).await.unwrap();
    let written = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            if let Ok(text) = message.to_text() {
                let result: Value = serde_json::from_str(text).unwrap();
                if result["id"] == 131 { break result; }
            }
        }
    }).await.unwrap();
    assert_eq!(written["ok"],true);
    std::fs::write(root.join("once.txt"),"changed externally").unwrap();
    let mut duplicate = create.clone();
    duplicate["id"] = json!(132);
    ws.send(Message::Text(duplicate.to_string().into())).await.unwrap();
    let recovered = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            if let Ok(text) = message.to_text() {
                let result: Value = serde_json::from_str(text).unwrap();
                if result["id"] == 132 { break result; }
            }
        }
    }).await.unwrap();
    assert_eq!(recovered["ok"],true);
    assert_eq!(std::fs::read_to_string(root.join("once.txt")).unwrap(),"changed externally");
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
