//! Emit actual payload-builder results for validation against the installed Codex schema.
use coding_agent_desktop::protocol::{self, RequestId};
use serde_json::json;
fn main() {
    let id = RequestId::Number(1);
    let mut samples = Vec::new();
    for (method, params) in [
        ("initialize", protocol::initialize_params()),
        (
            "thread/start",
            protocol::thread_params("/tmp/workspace", None, None, false),
        ),
        (
            "thread/resume",
            protocol::thread_params("/tmp/workspace", None, Some("t1"), true),
        ),
        (
            "turn/start",
            protocol::turn_params("t1", "Explain this repository"),
        ),
        ("turn/interrupt", json!({"threadId":"t1","turnId":"u1"})),
    ] {
        samples.push(
            json!({"schema":"ClientRequest.json","value":protocol::request(&id,method,params)}),
        );
    }
    samples.push(json!({"schema":"ClientNotification.json","value":protocol::notification("initialized",json!({}))}));
    samples.push(json!({"schema":"CommandExecutionRequestApprovalResponse.json","value":protocol::decision(&id,true)["result"]}));
    samples.push(json!({"schema":"FileChangeRequestApprovalResponse.json","value":protocol::decision(&id,false)["result"]}));
    println!("{}", serde_json::to_string_pretty(&samples).unwrap());
}
