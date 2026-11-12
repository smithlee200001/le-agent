//! Supervised child process and bidirectional RPC. No subprocess I/O runs on GPUI.
use crate::{
    protocol::{self, Message, RequestId},
    state::{Approval, Event},
};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command as ProcessCommand},
    sync::mpsc,
    time::{Instant, timeout},
};

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const RPC_TIMEOUT: Duration = Duration::from_secs(45);
#[derive(Clone, Debug)]
pub struct Config {
    pub executable: PathBuf,
    pub workspace: PathBuf,
    pub model: Option<String>,
    pub resume: Option<String>,
    pub read_only: bool,
}
impl Config {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            executable: "codex".into(),
            workspace,
            model: None,
            resume: None,
            read_only: false,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Command {
    SendPrompt(String),
    Interrupt,
    Decide { id: RequestId, accept: bool },
    Shutdown,
}
pub struct Controller {
    tx: mpsc::Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Controller {
    pub fn spawn(mut config: Config) -> Result<Self, String> {
        config.workspace = std::fs::canonicalize(&config.workspace)
            .map_err(|e| format!("Invalid workspace: {e}"))?;
        if !config.workspace.is_dir() {
            return Err("Workspace must be a directory".into());
        }
        // Resolve explicit relative binary paths before changing the child's cwd.
        if config.executable.components().count() > 1 && config.executable.is_relative() {
            config.executable = std::fs::canonicalize(&config.executable)
                .map_err(|e| format!("Invalid executable: {e}"))?;
        }
        let (tx, rx) = mpsc::channel(32);
        let (events_tx, events) = async_channel::bounded(512);
        let worker = std::thread::Builder::new()
            .name("codex-app-server".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(async {
                        let result = run(config, rx, events_tx.clone()).await;
                        if let Err(error) = &result {
                            let _ = events_tx.send(Event::Error(error.clone())).await;
                        }
                        let _ = events_tx
                            .send(Event::Disconnected(
                                result.err().unwrap_or_else(|| "App server stopped".into()),
                            ))
                            .await;
                    }),
                    Err(error) => {
                        let _ = events_tx.send_blocking(Event::Disconnected(error.to_string()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            events,
            worker: Some(worker),
        })
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.tx
            .try_send(command)
            .map_err(|e| format!("Backend unavailable or busy: {e}"))
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.tx.try_send(Command::Shutdown);
        self.events.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn emit(events: &async_channel::Sender<Event>, event: Event) -> Result<(), String> {
    timeout(Duration::from_secs(5), events.send(event))
        .await
        .map_err(|_| "UI stopped consuming events".to_owned())?
        .map_err(|_| "UI closed".into())
}
/// Read a bounded JSONL frame without first allocating an unbounded line.
async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|e| e.to_string())?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err("Truncated JSONL frame at EOF".into())
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline
            .map(|position| position + 1)
            .unwrap_or(available.len());
        if frame.len() + consumed > MAX_FRAME_BYTES {
            return Err("App server frame exceeds 8 MiB limit".into());
        }
        frame.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(frame));
        }
    }
}
struct Pending {
    method: String,
    deadline: Instant,
}
struct Session {
    stdin: ChildStdin,
    next_id: i64,
    pending: HashMap<RequestId, Pending>,
    approvals: HashMap<RequestId, Approval>,
    thread: Option<String>,
    turn: Option<String>,
    busy: bool,
    ready: bool,
    interrupt_queued: bool,
}
impl Session {
    async fn write(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        timeout(Duration::from_secs(5), async {
            self.stdin.write_all(&bytes).await?;
            self.stdin.flush().await
        })
        .await
        .map_err(|_| "App server stdin timed out".to_owned())?
        .map_err(|e| e.to_string())
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<(), String> {
        let id = RequestId::Number(self.next_id);
        self.next_id += 1;
        self.write(protocol::request(&id, method, params)).await?;
        self.pending.insert(
            id,
            Pending {
                method: method.into(),
                deadline: Instant::now() + RPC_TIMEOUT,
            },
        );
        Ok(())
    }
    async fn command(
        &mut self,
        command: Command,
        events: &async_channel::Sender<Event>,
    ) -> Result<bool, String> {
        match command {
            Command::Shutdown => return Ok(false),
            Command::SendPrompt(text) => {
                if !self.ready
                    || self.busy
                    || self.pending.values().any(|p| p.method == "turn/start")
                {
                    emit(
                        events,
                        Event::Error("Wait for the current turn or connection to finish".into()),
                    )
                    .await?;
                    emit(events, Event::Busy(self.busy)).await?;
                    return Ok(true);
                }
                if text.trim().is_empty() {
                    return Ok(true);
                }
                if text.len() > 1024 * 1024 {
                    emit(events, Event::Error("Prompt exceeds 1 MiB limit".into())).await?;
                    return Ok(true);
                }
                self.busy = true;
                self.interrupt_queued = false;
                emit(events, Event::Busy(true)).await?;
                self.request(
                    "turn/start",
                    protocol::turn_params(self.thread.as_deref().ok_or("No active thread")?, &text),
                )
                .await?;
            }
            Command::Interrupt => {
                if let (Some(thread), Some(turn)) = (&self.thread, &self.turn) {
                    if !self.pending.values().any(|p| p.method == "turn/interrupt") {
                        self.request("turn/interrupt", json!({"threadId":thread,"turnId":turn}))
                            .await?;
                    }
                } else if self.busy {
                    self.interrupt_queued = true;
                    emit(
                        events,
                        Event::Log("Interrupt queued until the turn ID arrives.".into()),
                    )
                    .await?;
                }
            }
            Command::Decide { id, accept } => {
                let Some(approval) = self.approvals.get(&id) else {
                    emit(
                        events,
                        Event::Log(
                            "Approval already resolved or expired; no decision sent.".into(),
                        ),
                    )
                    .await?;
                    return Ok(true);
                };
                if !protocol::allows_decision(&approval.params, accept) {
                    emit(
                        events,
                        Event::Error(
                            "Server does not offer this approval decision. Stop the turn instead."
                                .into(),
                        ),
                    )
                    .await?;
                    return Ok(true);
                }
                self.write(protocol::decision(&id, accept)).await?;
                self.approvals.remove(&id);
                emit(events, Event::ApprovalResolved(id)).await?;
            }
        }
        Ok(true)
    }
    async fn incoming(
        &mut self,
        message: Message,
        config: &Config,
        events: &async_channel::Sender<Event>,
    ) -> Result<(), String> {
        match message {
            Message::Response { id, result } => {
                let Some(pending) = self.pending.remove(&id) else {
                    emit(
                        events,
                        Event::Log(format!("Ignored response with unknown ID: {id:?}")),
                    )
                    .await?;
                    return Ok(());
                };
                let value = match result {
                    Ok(value) => value,
                    Err(error) => {
                        if pending.method == "initialize" || pending.method.starts_with("thread/") {
                            return Err(format!("{} failed: {}", pending.method, error));
                        }
                        if pending.method == "turn/start" {
                            self.busy = false;
                            emit(events, Event::Busy(false)).await?;
                        }
                        emit(
                            events,
                            Event::Error(format!("{} failed: {}", pending.method, error)),
                        )
                        .await?;
                        return Ok(());
                    }
                };
                match pending.method.as_str() {
                    "initialize" => {
                        self.write(protocol::notification("initialized", json!({})))
                            .await?;
                        emit(events, Event::Connected).await?;
                        self.request(
                            if config.resume.is_some() {
                                "thread/resume"
                            } else {
                                "thread/start"
                            },
                            protocol::thread_params(
                                &config.workspace.to_string_lossy(),
                                config.model.as_deref(),
                                config.resume.as_deref(),
                                config.read_only,
                            ),
                        )
                        .await?;
                    }
                    "thread/start" | "thread/resume" => {
                        let id = value["thread"]["id"]
                            .as_str()
                            .ok_or("Thread response is missing thread.id")?
                            .to_owned();
                        self.thread = Some(id);
                        self.ready = true;
                        emit(
                            events,
                            Event::ThreadOpened {
                                thread: value["thread"].clone(),
                                resumed: config.resume.is_some(),
                            },
                        )
                        .await?;
                    }
                    "turn/start" => {
                        // Notifications may arrive before the response. Never resurrect a completed turn.
                        if self.busy && self.turn.is_none() {
                            self.turn = value["turn"]["id"].as_str().map(str::to_owned);
                        }
                        if self.busy && self.turn.is_none() {
                            return Err("Turn response is missing turn.id".into());
                        }
                    }
                    "turn/interrupt" => {
                        emit(
                            events,
                            Event::Log("Interrupt requested; awaiting turn/completed.".into()),
                        )
                        .await?;
                    }
                    _ => {}
                }
            }
            Message::Request { id, method, params } => {
                if protocol::is_approval(&method) {
                    if self.approvals.contains_key(&id) {
                        return Err("Server reused an unresolved approval ID; connection closed without approving".into());
                    }
                    if self.approvals.len() >= 64 {
                        self.write(protocol::decision(&id, false)).await?;
                        emit(
                            events,
                            Event::Error(
                                "Too many pending approvals; declined overflow request.".into(),
                            ),
                        )
                        .await?;
                        return Ok(());
                    }
                    let approval = Approval {
                        id: id.clone(),
                        method,
                        params,
                    };
                    self.approvals.insert(id, approval.clone());
                    emit(events, Event::ApprovalRequested(approval)).await?;
                } else {
                    self.write(protocol::unsupported(&id, &method)).await?;
                    emit(events,Event::Error(format!("Unsupported server request: {method}. No permission was granted. Stop this turn if it cannot continue."))).await?;
                }
            }
            Message::Notification { method, params } => {
                // RPC request IDs are connection-wide, including approvals from child threads.
                if method == "serverRequest/resolved"
                    && let Ok(id) = serde_json::from_value::<RequestId>(params["requestId"].clone())
                {
                    self.approvals.remove(&id);
                    emit(events, Event::ApprovalResolved(id)).await?;
                }
                let for_thread = params["threadId"]
                    .as_str()
                    .is_none_or(|id| self.thread.as_deref().is_none_or(|active| id == active));
                if for_thread {
                    match method.as_str() {
                        "turn/started" => {
                            self.turn = params["turn"]["id"].as_str().map(str::to_owned);
                            self.busy = true;
                        }
                        "turn/completed"
                            if params["turn"]["id"].as_str().is_none_or(|id| {
                                self.turn.as_deref().is_none_or(|active| id == active)
                            }) =>
                        {
                            self.turn = None;
                            self.busy = false;
                            self.approvals.clear();
                        }
                        _ => {}
                    }
                }
                emit(events, Event::Notification { method, params }).await?;
            }
        }
        if self.interrupt_queued
            && self.busy
            && !self.pending.values().any(|p| p.method == "turn/interrupt")
            && let (Some(thread), Some(turn)) = (&self.thread, &self.turn)
        {
            self.request("turn/interrupt", json!({"threadId":thread,"turnId":turn}))
                .await?;
            self.interrupt_queued = false;
        }
        Ok(())
    }
}
async fn run(
    config: Config,
    mut commands: mpsc::Receiver<Command>,
    events: async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut command = ProcessCommand::new(&config.executable);
    command
        .arg("app-server")
        .current_dir(&config.workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| {
        format!(
            "Cannot launch {} app-server: {e}. Install Codex CLI and run codex login first.",
            config.executable.display()
        )
    })?;
    let mut session = Session {
        stdin: child.stdin.take().ok_or("Missing child stdin")?,
        next_id: 1,
        pending: HashMap::new(),
        approvals: HashMap::new(),
        thread: None,
        turn: None,
        busy: false,
        ready: false,
        interrupt_queued: false,
    };
    let stdout = child.stdout.take().ok_or("Missing child stdout")?;
    let stderr = child.stderr.take().ok_or("Missing child stderr")?;
    let (wire_tx, mut wire_rx) = mpsc::channel(256);
    let reader = tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        loop {
            let result = read_frame(&mut stdout).await;
            let done = !matches!(result, Ok(Some(_)));
            if wire_tx.send(result).await.is_err() || done {
                break;
            }
        }
    });
    // stderr is diagnostic-only, never parsed as protocol; bound each read and total log count.
    let stderr_events = events.clone();
    let stderr_reader = tokio::spawn(async move {
        let mut stderr = BufReader::new(stderr);
        while let Ok(Some(line)) = read_frame(&mut stderr).await {
            let text = String::from_utf8_lossy(&line);
            let mut message = text.chars().take(4096).collect::<String>();
            if text.len() > 4096 {
                message.push_str(" [truncated]");
            }
            if emit(
                &stderr_events,
                Event::Log(format!("stderr: {}", message.trim_end())),
            )
            .await
            .is_err()
            {
                break;
            }
        }
    });
    let result=async {
        session.request("initialize",protocol::initialize_params()).await?;
        let mut timer=tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                command=commands.recv()=>{match command {Some(command)=>if !session.command(command,&events).await? {break;},None=>break}},
                line=wire_rx.recv()=>{
                    match line {
                        Some(Ok(Some(line)))=>{
                            if line.iter().all(u8::is_ascii_whitespace) {continue;}
                            session.incoming(protocol::decode(&line).map_err(|e|format!("Malformed app-server JSONL: {e}"))?,&config,&events).await?;
                        }
                        Some(Err(error))=>return Err(error),
                        Some(Ok(None)) | None=>return Err("App server closed stdout".into()),
                    }
                }
                status=child.wait()=>return Err(format!("App server exited: {}",status.map_err(|e|e.to_string())?)),
                _=timer.tick()=>{
                    if let Some(pending)=session.pending.values().find(|pending|pending.deadline<=Instant::now()) {return Err(format!("{} timed out. Connection closed; outcome may be uncertain. No automatic retry.",pending.method));}
                    if events.is_closed() {break;}
                }
            }
        }
        Ok(())
    }.await;
    reader.abort();
    stderr_reader.abort();
    // Closing stdin allows Codex to flush history and clean up its own tool children.
    // A separate process is not a sandbox or a guarantee about every descendant.
    drop(session);
    if timeout(Duration::from_secs(2), child.wait()).await.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn frames_are_newline_delimited_and_bounded() {
        let mut reader = BufReader::new(&b"{\"a\":1}\n{}\n"[..]);
        assert_eq!(
            read_frame(&mut reader).await.unwrap().unwrap(),
            b"{\"a\":1}\n"
        );
        assert_eq!(read_frame(&mut reader).await.unwrap().unwrap(), b"{}\n");
        assert!(read_frame(&mut reader).await.unwrap().is_none());
        let bytes = vec![b'x'; MAX_FRAME_BYTES + 1];
        let mut reader = BufReader::new(bytes.as_slice());
        assert!(read_frame(&mut reader).await.is_err());
    }
    #[tokio::test]
    async fn eof_mid_frame_is_error() {
        let mut reader = BufReader::new(&b"{}"[..]);
        assert!(read_frame(&mut reader).await.is_err());
    }
}
