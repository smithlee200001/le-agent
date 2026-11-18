use coding_agent_desktop::{
    engine::{Command, Config, Controller},
    state::{AppState, Event},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn mock_config(workspace: PathBuf) -> Config {
    let mut config = Config::new(workspace);
    // Windows does not execute Python shebangs. Use an explicit interpreter on
    // every platform, with an override for CI and local virtual environments.
    config.executable = std::env::var_os("PYTHON")
        .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into())
        .into();
    config.executable_args = vec![
        "-X".into(),
        "utf8".into(),
        "-u".into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/mock_app_server.py")
            .into_os_string(),
    ];
    config
}
fn start() -> Controller {
    Controller::spawn(mock_config(PathBuf::from(env!("CARGO_MANIFEST_DIR")))).unwrap()
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
    let controller = Controller::spawn(mock_config(dir.clone())).unwrap();
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

#[test]
fn interrupt_after_ack_before_terminal_is_not_sent_twice() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("interrupt-gated".into()))
        .unwrap();
    until(&controller, &mut state, |s| s.turn_id.is_some());
    controller.send(Command::Interrupt).unwrap();
    until(&controller, &mut state, |s| {
        !s.approvals.is_empty() && s.logs.iter().any(|l| l.contains("awaiting turn/completed"))
    });
    assert!(state.busy, "ACK must not finish the turn");
    assert!(state.cancelling);
    assert_eq!(state.status, "Cancelling…");
    controller.send(Command::Interrupt).unwrap();
    controller
        .send(Command::Decide {
            id: state.approvals[0].id.clone(),
            accept: false,
        })
        .unwrap();
    until(&controller, &mut state, |s| s.status == "Turn interrupted");
}

#[test]
fn old_turn_events_cannot_mutate_new_turn_or_confirmed_history() {
    use serde_json::json;
    let mut state = AppState::default();
    let notify = |state: &mut AppState, method: &str, params| {
        state.apply(Event::Notification {
            method: method.into(),
            params,
        })
    };
    notify(
        &mut state,
        "turn/started",
        json!({"threadId":"t", "turn":{"id":"a"}}),
    );
    notify(
        &mut state,
        "item/completed",
        json!({"threadId":"t", "turnId":"a", "item":{"id":"history", "type":"agentMessage", "text":"confirmed"}}),
    );
    notify(
        &mut state,
        "turn/completed",
        json!({"threadId":"t", "turn":{"id":"a", "status":"completed"}}),
    );
    state.apply(Event::Busy(true));
    // Old frames can also arrive before B's identity is known.
    notify(
        &mut state,
        "item/agentMessage/delta",
        json!({"threadId":"t", "turnId":"a", "itemId":"history", "delta":"stale"}),
    );
    notify(
        &mut state,
        "turn/started",
        json!({"threadId":"t", "turn":{"id":"b"}}),
    );
    notify(
        &mut state,
        "turn/diff/updated",
        json!({"threadId":"t", "turnId":"b", "diff":"new diff"}),
    );
    for method in [
        "item/started",
        "item/completed",
        "item/agentMessage/delta",
        "item/commandExecution/outputDelta",
        "turn/diff/updated",
        "turn/completed",
        "turn/started",
    ] {
        notify(
            &mut state,
            method,
            json!({"threadId":"t", "turnId":"a", "itemId":"history", "delta":"stale", "diff":"old diff", "item":{"id":"history", "type":"agentMessage", "text":"stale"}, "turn":{"id":"a", "status":"completed"}}),
        );
    }
    assert_eq!(state.items[0].text, "confirmed");
    assert_eq!(state.turn_id.as_deref(), Some("b"));
    assert!(state.busy);
    assert_eq!(state.diff, "new diff");
}

#[test]
fn transport_ignores_retired_turns_before_and_after_new_identity() {
    let controller = start();
    let mut state = AppState::default();
    until(&controller, &mut state, |s| s.connected);
    controller
        .send(Command::SendPrompt("history-a".into()))
        .unwrap();
    until(&controller, &mut state, |s| s.status == "Turn completed");
    controller
        .send(Command::SendPrompt("stale-b".into()))
        .unwrap();
    until(&controller, &mut state, |s| {
        s.items.iter().any(|i| i.text == "B ready")
    });
    assert_eq!(state.items[0].text, "confirmed");
    assert_eq!(state.turn_id.as_deref(), Some("turn-2"));
    assert!(state.busy);
    assert!(state.diff.is_empty());
    controller.send(Command::Interrupt).unwrap();
    until(&controller, &mut state, |s| s.status == "Turn interrupted");
}

#[test]
fn cancellation_before_identity_and_start_failure_are_cleaned_up() {
    for prompt in [
        "start-gated",
        "start-gated-failure",
        "start-gated-completed",
    ] {
        let controller = start();
        let mut state = AppState::default();
        until(&controller, &mut state, |s| s.connected);
        controller.send(Command::SendPrompt(prompt.into())).unwrap();
        until(&controller, &mut state, |s| !s.approvals.is_empty());
        assert!(state.turn_id.is_none());
        controller.send(Command::Interrupt).unwrap();
        until(&controller, &mut state, |s| s.cancelling);
        controller.send(Command::Interrupt).unwrap();
        controller
            .send(Command::Decide {
                id: state.approvals[0].id.clone(),
                accept: false,
            })
            .unwrap();
        until(&controller, &mut state, |s| !s.busy);
        if prompt == "start-gated-completed" {
            // This gate is emitted after the start response, so the next prompt
            // cannot race the worker removing its pending turn/start request.
            until(&controller, &mut state, |s| !s.approvals.is_empty());
            controller
                .send(Command::Decide {
                    id: state.approvals[0].id.clone(),
                    accept: false,
                })
                .unwrap();
            until(&controller, &mut state, |s| s.approvals.is_empty());
        }
        assert!(!state.cancelling);
        assert!(state.turn_id.is_none());
        assert!(state.approvals.is_empty());
        if prompt.ends_with("failure") {
            assert!(state.last_error.as_ref().unwrap().contains("start failed"));
        }
        // No queued cancellation leaks into the next turn.
        controller
            .send(Command::SendPrompt("history-a".into()))
            .unwrap();
        until(&controller, &mut state, |s| {
            s.items.iter().any(|i| i.text == "confirmed")
        });
    }
}

#[test]
fn cancellation_clears_on_normal_completion_failure_and_disconnect() {
    for (prompt, status) in [
        ("interrupt-gated-completed", "Turn completed"),
        ("interrupt-gated-failed", "Turn failed"),
        ("interrupt-gated-disconnect", "Disconnected"),
    ] {
        let controller = start();
        let mut state = AppState::default();
        until(&controller, &mut state, |s| s.connected);
        controller.send(Command::SendPrompt(prompt.into())).unwrap();
        until(&controller, &mut state, |s| s.turn_id.is_some());
        controller.send(Command::Interrupt).unwrap();
        until(&controller, &mut state, |s| {
            !s.approvals.is_empty() && s.logs.iter().any(|l| l.contains("awaiting turn/completed"))
        });
        assert!(state.busy && state.cancelling);
        controller
            .send(Command::Decide {
                id: state.approvals[0].id.clone(),
                accept: false,
            })
            .unwrap();
        until(&controller, &mut state, |s| s.status == status);
        assert!(!state.busy && !state.cancelling);
        assert!(state.turn_id.is_none() && state.approvals.is_empty());
        if state.connected {
            controller.send(Command::Interrupt).unwrap();
            controller
                .send(Command::SendPrompt("history-a".into()))
                .unwrap();
            until(&controller, &mut state, |s| {
                s.items.iter().any(|i| i.text == "confirmed")
            });
            assert!(state.connected);
        }
    }
}

#[test]
fn resumed_history_and_current_turn_out_of_order_events_are_preserved() {
    use serde_json::json;
    let mut state = AppState::default();
    state.apply(Event::ThreadOpened { resumed: true, thread: json!({"id":"t", "turns":[
        {"id":"a", "status":"completed", "items":[{"id":"history", "type":"agentMessage", "text":"confirmed"}]},
        {"id":"b", "status":"inProgress", "items":[]}
    ]}) });
    let notify = |state: &mut AppState, method: &str, params| {
        state.apply(Event::Notification {
            method: method.into(),
            params,
        })
    };
    notify(
        &mut state,
        "item/agentMessage/delta",
        json!({"threadId":"t", "turnId":"a", "itemId":"history", "delta":"stale"}),
    );
    notify(
        &mut state,
        "item/agentMessage/delta",
        json!({"threadId":"t", "turnId":"b", "itemId":"current", "delta":"early"}),
    );
    notify(
        &mut state,
        "turn/diff/updated",
        json!({"threadId":"t", "turnId":"b", "diff":"early diff"}),
    );
    notify(
        &mut state,
        "turn/started",
        json!({"threadId":"t", "turn":{"id":"b"}}),
    );
    assert_eq!(state.items[0].text, "confirmed");
    assert_eq!(state.items[1].text, "early");
    assert_eq!(state.diff, "early diff");
    assert!(state.busy);
    notify(
        &mut state,
        "item/completed",
        json!({"threadId":"t", "turnId":"b", "item":{"id":"current", "type":"agentMessage", "text":"final"}}),
    );
    notify(
        &mut state,
        "turn/completed",
        json!({"threadId":"t", "turn":{"id":"b", "status":"completed"}}),
    );
    notify(
        &mut state,
        "turn/started",
        json!({"threadId":"t", "turn":{"id":"b"}}),
    );
    assert!(!state.busy);
    assert_eq!(state.items[1].text, "final");
}
