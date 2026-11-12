//! UI-independent state reducer. Transport and all process I/O stay off the UI thread.
use crate::protocol::RequestId;
use serde_json::Value;

const MAX_ITEMS: usize = 2000;
const MAX_TEXT: usize = 256 * 1024;
#[derive(Clone, Debug)]
pub struct Approval {
    pub id: RequestId,
    pub method: String,
    pub params: Value,
}
#[derive(Clone, Debug, Default)]
pub struct ItemView {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub status: String,
}
#[derive(Clone, Debug)]
pub enum Event {
    Connected,
    ThreadOpened { thread: Value, resumed: bool },
    Notification { method: String, params: Value },
    ApprovalRequested(Approval),
    ApprovalResolved(RequestId),
    Busy(bool),
    Log(String),
    Error(String),
    Disconnected(String),
}
#[derive(Default)]
pub struct AppState {
    pub status: String,
    pub connected: bool,
    pub busy: bool,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub items: Vec<ItemView>,
    pub approvals: Vec<Approval>,
    pub logs: Vec<String>,
    pub diff: String,
    pub last_error: Option<String>,
}
fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
fn truncate(text: &mut String) {
    if text.len() > MAX_TEXT {
        let mut boundary = MAX_TEXT;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push_str("\n[display truncated]");
    }
}
impl AppState {
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Connected => self.status = "Connecting to thread…".into(),
            Event::ThreadOpened { thread, resumed } => {
                self.thread_id = thread["id"].as_str().map(str::to_owned);
                self.connected = true;
                self.status = "Ready".into();
                if let Some(turns) = thread["turns"].as_array() {
                    for turn in turns {
                        if let Some(items) = turn["items"].as_array() {
                            for item in items {
                                self.upsert(item);
                            }
                        }
                    }
                }
                if resumed {
                    self.log("Resumed thread. This MVP displays inline history returned by the server; older/paginated history may be absent.".into());
                }
            }
            Event::Busy(busy) => {
                self.busy = busy;
                self.status = if busy { "Working" } else { "Ready" }.into();
                if busy {
                    self.last_error = None;
                }
            }
            Event::ApprovalRequested(approval) => {
                if !self.approvals.iter().any(|a| a.id == approval.id) {
                    self.approvals.push(approval);
                }
                self.status = "Approval required".into();
            }
            Event::ApprovalResolved(id) => {
                self.approvals.retain(|a| a.id != id);
                self.status = if self.busy { "Working" } else { "Ready" }.into();
            }
            Event::Log(log) => self.log(log),
            Event::Error(error) => {
                self.last_error = Some(error.clone());
                self.log(error);
            }
            Event::Disconnected(reason) => {
                self.connected = false;
                self.busy = false;
                self.turn_id = None;
                self.approvals.clear();
                self.status = "Disconnected".into();
                self.log(reason);
            }
            Event::Notification { method, params } => self.notification(&method, &params),
        }
    }
    fn log(&mut self, mut message: String) {
        truncate(&mut message);
        self.logs.push(message);
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
    }
    fn notification(&mut self, method: &str, params: &Value) {
        if let Some(id) = params["threadId"].as_str()
            && self.thread_id.as_deref().is_some_and(|active| active != id)
        {
            return;
        }
        match method {
            "turn/started" => {
                self.turn_id = params["turn"]["id"].as_str().map(str::to_owned);
                self.busy = true;
                self.status = "Working".into();
                self.diff.clear();
            }
            "turn/completed" => {
                if let Some(id) = params["turn"]["id"].as_str()
                    && self.turn_id.as_deref().is_some_and(|active| active != id)
                {
                    return;
                }
                self.busy = false;
                self.turn_id = None;
                self.approvals.clear();
                self.status = format!(
                    "Turn {}",
                    params["turn"]["status"].as_str().unwrap_or("completed")
                );
                if let Some(error) = params["turn"].get("error").filter(|v| !v.is_null()) {
                    self.last_error = Some(error.to_string());
                }
            }
            "item/started" | "item/completed" => self.upsert(&params["item"]),
            "item/agentMessage/delta" | "item/commandExecution/outputDelta" => {
                let id = string(params, "itemId");
                if id.is_empty() {
                    return;
                }
                if !self.items.iter().any(|item| item.id == id) {
                    self.items.push(ItemView {
                        id: id.clone(),
                        kind: if method.contains("agentMessage") {
                            "agentMessage"
                        } else {
                            "commandExecution"
                        }
                        .into(),
                        ..Default::default()
                    });
                }
                if let Some(item) = self.items.iter_mut().find(|item| item.id == id)
                    && item.text.len() <= MAX_TEXT
                {
                    item.text
                        .push_str(params["delta"].as_str().unwrap_or_default());
                    truncate(&mut item.text);
                }
            }
            "turn/diff/updated" => {
                self.diff = string(params, "diff");
                truncate(&mut self.diff);
            }
            "error" => {
                let error = params["error"].to_string();
                self.last_error = Some(error.clone());
                self.log(error);
            }
            _ => {}
        }
        if self.items.len() > MAX_ITEMS {
            self.items.drain(..self.items.len() - MAX_ITEMS);
        }
    }
    fn upsert(&mut self, item: &Value) {
        let id = string(item, "id");
        if id.is_empty() {
            return;
        }
        let kind = string(item, "type");
        let mut text = match kind.as_str() {
            "agentMessage" => string(item, "text"),
            "userMessage" => item["content"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .filter_map(|part| part["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            "commandExecution" => format!(
                "$ {}\n{}",
                string(item, "command"),
                string(item, "aggregatedOutput")
            ),
            "fileChange" => serde_json::to_string_pretty(&item["changes"]).unwrap_or_default(),
            _ => serde_json::to_string_pretty(item).unwrap_or_default(),
        };
        truncate(&mut text);
        let view = ItemView {
            id: id.clone(),
            kind,
            text,
            status: string(item, "status"),
        };
        if let Some(existing) = self.items.iter_mut().find(|existing| existing.id == id) {
            *existing = view;
        } else {
            self.items.push(view);
        }
        if self.items.len() > MAX_ITEMS {
            self.items.remove(0);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn notify(state: &mut AppState, method: &str, params: Value) {
        state.apply(Event::Notification {
            method: method.into(),
            params,
        });
    }
    #[test]
    fn delta_reconciles_with_completed_item() {
        let mut s = AppState::default();
        notify(
            &mut s,
            "item/agentMessage/delta",
            json!({"itemId":"i","delta":"你"}),
        );
        notify(
            &mut s,
            "item/agentMessage/delta",
            json!({"itemId":"i","delta":"好"}),
        );
        assert_eq!(s.items[0].text, "你好");
        notify(
            &mut s,
            "item/completed",
            json!({"item":{"id":"i","type":"agentMessage","text":"你好!"}}),
        );
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.items[0].text, "你好!");
    }
    #[test]
    fn cancellation_waits_for_completed_and_ignores_other_threads() {
        let mut s = AppState {
            thread_id: Some("t".into()),
            ..Default::default()
        };
        notify(
            &mut s,
            "turn/started",
            json!({"threadId":"t","turn":{"id":"u"}}),
        );
        assert!(s.busy);
        notify(
            &mut s,
            "turn/completed",
            json!({"threadId":"other","turn":{"id":"u"}}),
        );
        assert!(s.busy);
        notify(
            &mut s,
            "turn/completed",
            json!({"threadId":"t","turn":{"id":"u","status":"interrupted"}}),
        );
        assert!(!s.busy);
        assert!(s.status.contains("interrupted"));
    }
    #[test]
    fn disconnect_clears_approval_and_execution_state() {
        let mut s = AppState::default();
        s.apply(Event::ApprovalRequested(Approval {
            id: RequestId::Number(1),
            method: "m".into(),
            params: Value::Null,
        }));
        s.apply(Event::Disconnected("exit".into()));
        assert!(s.approvals.is_empty());
        assert!(!s.busy);
    }
}
