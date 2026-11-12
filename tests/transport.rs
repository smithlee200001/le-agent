#![cfg(unix)]
use coding_agent_desktop::{
    engine::{Command, Config, Controller},
    state::{AppState, Event},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn start() -> Controller {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut config = Config::new(root.clone());
    config.executable = root.join("tests/mock_app_server.py");
    Controller::spawn(config).unwrap()
}
fn until(controller: &Controller, state: &mut AppState, condition: impl Fn(&AppState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition(state) {
        while let Ok(event) = controller.events.try_recv() {
            state.apply(event);
        }
        assert!(
            Instant::now() < deadline,
            "Timed out: status={}, error={:?}, logs={:?}",
            state.status,
            state.last_error,
            state.logs
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn handshake_streaming_bidirectional_ids_approval_and_repeated_turns() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    for accept in [true, false] {
        controller.send(Command::SendPrompt("test".into())).unwrap();
        until(&controller, &mut state, |s| !s.approvals.is_empty());
        let id = state.approvals[0].id.clone();
        controller
            .send(Command::Decide {
                id: id.clone(),
                accept,
            })
            .unwrap();
        // Duplicate click must not send a second wire response.
        controller.send(Command::Decide { id, accept }).unwrap();
        until(&controller, &mut state, |s| {
            !s.busy && s.approvals.is_empty()
        });
        assert!(state.items.iter().any(|item| item.text == "你好，完成。"));
        assert!(state.diff.contains("+hello"));
    }
    controller.send(Command::Shutdown).unwrap();
    until(&controller, &mut state, |s| !s.connected);
}
#[test]
fn interrupt_completes_only_on_notification() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("interrupt".into()))
        .unwrap();
    until(&controller, &mut state, |s| s.turn_id.is_some());
    controller.send(Command::Interrupt).unwrap();
    until(&controller, &mut state, |s| s.status == "Turn interrupted");
    assert!(!state.busy);
}
#[test]
fn unsupported_request_gets_rpc_error_instead_of_hanging() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("unsupported".into()))
        .unwrap();
    until(&controller, &mut state, |s| s.status == "Turn completed");
    assert!(
        state
            .last_error
            .as_ref()
            .unwrap()
            .contains("Unsupported server request")
    );
}
#[test]
fn crash_disconnects_and_clears_execution() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("crash".into()))
        .unwrap();
    until(&controller, &mut state, |s| s.status == "Disconnected");
    assert!(!state.busy);
}
#[test]
fn malformed_frame_is_terminal_not_silently_ignored() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("malformed".into()))
        .unwrap();
    until(&controller, &mut state, |s| s.status == "Disconnected");
    assert!(state.last_error.unwrap().contains("Malformed"));
}
#[test]
fn errors_are_not_automatically_final_turn_events() {
    let mut state = AppState::default();
    state.apply(Event::Busy(true));
    state.apply(Event::Notification {
        method: "error".into(),
        params: serde_json::json!({"error":{"message":"retrying"},"willRetry":true}),
    });
    assert!(state.busy);
}

#[test]
fn interrupt_immediately_after_send_is_queued() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("interrupt".into()))
        .unwrap();
    controller.send(Command::Interrupt).unwrap();
    until(&controller, &mut state, |s| s.status == "Turn interrupted");
    assert!(!state.busy);
}

#[test]
fn child_thread_resolutions_remove_approval_by_request_id() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("child-approval".into()))
        .unwrap();
    until(&controller, &mut state, |s| {
        s.items.iter().any(|i| i.text == "child resolved")
    });
    assert!(state.approvals.is_empty());
    controller
        .send(Command::Decide {
            id: coding_agent_desktop::protocol::RequestId::String("child-request".into()),
            accept: true,
        })
        .unwrap();
    controller.send(Command::Interrupt).unwrap();
    until(&controller, &mut state, |s| s.status == "Turn interrupted");
}
#[test]
fn dropping_controller_allows_eof_cleanup_and_reaps_child() {
    let dir = std::env::temp_dir().join(format!(
        "coding-agent-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("record_cleanup"), b"yes").unwrap();
    let mut config = Config::new(dir.clone());
    config.executable = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/mock_app_server.py");
    let controller = Controller::spawn(config).unwrap();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    drop(controller);
    assert_eq!(
        std::fs::read_to_string(dir.join("cleanup_complete")).unwrap(),
        "done"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn duplicate_unresolved_approval_id_closes_connection_without_approval() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("duplicate-approval".into()))
        .unwrap();
    until(&controller, &mut state, |s| s.status == "Disconnected");
    assert!(
        state
            .last_error
            .unwrap()
            .contains("reused an unresolved approval ID")
    );
    assert!(state.approvals.is_empty());
}
