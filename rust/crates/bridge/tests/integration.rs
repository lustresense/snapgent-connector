use serde_json::json;
use snapgent_bridge::State;
use std::time::Duration;

#[tokio::test]
async fn native_bridge_lifecycle() {
    let root = std::env::temp_dir().join(format!("snapgent-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("config.json"),
        r#"{"mcpServers":{"zed":{},"vscode":{},"disabled_external":{"command":"invalid-never-start","enabled":false}}}"#,
    )
    .unwrap();
    let state = State::new(root.clone(), "ab".repeat(32), vec![]).unwrap();
    state.manager.start().await.unwrap();
    let snapshot = state.manager.snapshot().await;
    assert_eq!(snapshot["servers"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["servers"][0]["id"], "local");
    assert_eq!(snapshot["tools"].as_array().unwrap().len(), 28);
    assert!(
        state
            .manager
            .resolve("local/read_file", Some("vscode"))
            .await
            .is_err()
    );
    assert!(
        state
            .manager
            .resolve("not_a_real_tool", None)
            .await
            .is_err()
    );
    let approvals = state.approvals.clone();
    let pending =
        tokio::spawn(async move { approvals.ask("Test approval", json!({}), false).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    state.approvals.take().unwrap().respond(false);
    assert!(!pending.await.unwrap());
    state.request_stop();
    assert!(!state.approvals.ask("Must not approve after shutdown", json!({}), true).await);
    state.manager.stop().await;
    assert_eq!(state.manager.snapshot().await["servers"], json!([]));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn explicit_workspace_binding_is_idempotent_and_isolated() {
    use snapgent_workspace::{Editor, Workspace};
    let root = std::env::temp_dir().join(format!("snapgent-binding-{}", std::process::id()));
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let manager = snapgent_bridge::Manager::new(root.clone());
    let sa = "11".repeat(32);
    let sb = "22".repeat(32);
    manager
        .bind_session(&sa, "local", Workspace::bound(Editor::Local, &a).unwrap())
        .await
        .unwrap();
    manager
        .bind_session(&sa, "local", Workspace::bound(Editor::Local, &a).unwrap())
        .await
        .unwrap();
    assert!(
        manager
            .bind_session(&sa, "local", Workspace::bound(Editor::Local, &b).unwrap())
            .await
            .is_err()
    );
    manager
        .bind_session(&sb, "local", Workspace::bound(Editor::Local, &b).unwrap())
        .await
        .unwrap();
    for (sid, content) in [(&sa, "A"), (&sb, "B")] {
        let backend = manager.session_backend(sid, "local").await.unwrap();
        snapgent_bridge::Manager::call(
            backend,
            "write_file".into(),
            json!({"path":"identity.txt","content":content}),
            5,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(a.join("identity.txt")).unwrap(),
        "A"
    );
    assert_eq!(
        std::fs::read_to_string(b.join("identity.txt")).unwrap(),
        "B"
    );
    manager.release_session(&sa).await;
    assert!(manager.session_backend(&sa, "local").await.is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn project_config_is_not_native_execution_config() {
    let root = std::env::temp_dir().join(format!("snapgent-config-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.json"), "not valid json").unwrap();
    let private = root.join("private-mcp.json");
    let state = State::with_config(root.clone(), private.clone(), "ab".repeat(32), vec![]).unwrap();
    state.manager.start().await.unwrap();
    assert_eq!(state.manager.snapshot().await["servers"][0]["id"], "local");
    assert!(!private.exists());
    state.manager.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}
