use serde_json::json;
use snapgent_bridge::State;
use std::time::Duration;

#[tokio::test]
async fn native_bridge_lifecycle() {
    let root = std::env::temp_dir().join(format!("snapgent-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("config.json"),
        r#"{"mcpServers":{"zed":{},"vscode":{}}}"#,
    )
    .unwrap();
    let state = State::new(root.clone(), "ab".repeat(32), vec![]).unwrap();
    state.manager.start().await.unwrap();
    let snapshot = state.manager.snapshot().await;
    assert_eq!(snapshot["servers"].as_array().unwrap().len(), 2);
    assert!(
        state
            .manager
            .resolve("zed/read_file", Some("vscode"))
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
    state.manager.stop().await;
    assert_eq!(state.manager.snapshot().await["servers"], json!([]));
    std::fs::remove_dir_all(root).unwrap();
}
