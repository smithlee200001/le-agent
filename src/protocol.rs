//! Codex App Server uses JSON-RPC semantics with no `jsonrpc` wire member.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: RequestId,
        result: Result<Value, Value>,
    },
}

pub fn decode(line: &[u8]) -> Result<Message, String> {
    let v: Value = serde_json::from_slice(line).map_err(|e| e.to_string())?;
    let object = v.as_object().ok_or("RPC message must be an object")?;
    let id = object
        .get("id")
        .map(|v| serde_json::from_value::<RequestId>(v.clone()))
        .transpose()
        .map_err(|e| e.to_string())?;
    if let Some(method) = object.get("method") {
        let method = method.as_str().ok_or("method must be a string")?.to_owned();
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        return Ok(match id {
            Some(id) => Message::Request { id, method, params },
            None => Message::Notification { method, params },
        });
    }
    let id = id.ok_or("response has no id")?;
    let result = match (object.get("result"), object.get("error")) {
        (Some(value), None) => Ok(value.clone()),
        (None, Some(error)) => Err(error.clone()),
        _ => return Err("response requires exactly one of result/error".into()),
    };
    Ok(Message::Response { id, result })
}

pub fn request(id: &RequestId, method: &str, params: Value) -> Value {
    json!({"id":id,"method":method,"params":params})
}
pub fn notification(method: &str, params: Value) -> Value {
    json!({"method":method,"params":params})
}
pub fn decision(id: &RequestId, accept: bool) -> Value {
    json!({"id":id,"result":{"decision":if accept {"accept"} else {"decline"}}})
}
pub fn unsupported(id: &RequestId, method: &str) -> Value {
    json!({"id":id,"error":{"code":-32601,"message":format!("Unsupported server request: {method}")}})
}
pub fn initialize_params() -> Value {
    json!({"clientInfo":{"name":"coding_agent_desktop","title":"Coding Agent Desktop","version":env!("CARGO_PKG_VERSION")}})
}
pub fn thread_params(
    workspace: &str,
    model: Option<&str>,
    resume: Option<&str>,
    read_only: bool,
) -> Value {
    let mut params = json!({"cwd":workspace,"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":if read_only {"read-only"} else {"workspace-write"}});
    if let Some(model) = model {
        params["model"] = model.into();
    }
    if let Some(id) = resume {
        params["threadId"] = id.into();
    }
    params
}
pub fn turn_params(thread: &str, text: &str) -> Value {
    json!({"threadId":thread,"input":[{"type":"text","text":text,"text_elements":[]}]})
}
pub fn is_approval(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
    )
}
pub fn allows_decision(params: &Value, accept: bool) -> bool {
    let Some(choices) = params.get("availableDecisions").and_then(Value::as_array) else {
        return true;
    };
    choices
        .iter()
        .any(|choice| choice.as_str() == Some(if accept { "accept" } else { "decline" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_server_requests_from_notifications() {
        assert!(matches!(
            decode(br#"{"id":"7","method":"item/fileChange/requestApproval","params":{}}"#)
                .unwrap(),
            Message::Request {
                id: RequestId::String(_),
                ..
            }
        ));
        assert!(matches!(
            decode(br#"{"method":"turn/completed","params":{},"emittedAtMs":8}"#).unwrap(),
            Message::Notification { .. }
        ));
    }
    #[test]
    fn response_error_and_null() {
        assert!(matches!(
            decode(br#"{"id":1,"result":null}"#).unwrap(),
            Message::Response {
                result: Ok(Value::Null),
                ..
            }
        ));
        assert!(matches!(
            decode(br#"{"id":1,"error":{"code":-1}}"#).unwrap(),
            Message::Response { result: Err(_), .. }
        ));
        assert!(decode(br#"{"id":1,"error":{},"result":{}}"#).is_err());
    }
    #[test]
    fn wire_has_no_jsonrpc_and_preserves_id_type() {
        let value = decision(&RequestId::String("42".into()), false);
        assert_eq!(value["id"], "42");
        assert_eq!(value["result"]["decision"], "decline");
        assert!(
            request(&RequestId::Number(1), "initialize", initialize_params())
                .get("jsonrpc")
                .is_none()
        );
    }
    #[test]
    fn payload_matches_v2() {
        assert_eq!(
            thread_params("/repo", None, None, false)["sandbox"],
            "workspace-write"
        );
        assert_eq!(
            turn_params("t", "hello")["input"][0]["text_elements"],
            json!([])
        );
        assert!(!allows_decision(
            &json!({"availableDecisions":["decline"]}),
            true
        ));
    }
}
