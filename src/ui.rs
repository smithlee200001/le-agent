//! Small native GPUI shell. All protocol and subprocess work lives in `engine`.
use crate::{
    engine::{Command, Config, Controller},
    state::AppState,
    text_input::{self, TextInput},
};
use gpui::{prelude::*, *};
use std::time::Duration;

actions!(coding_agent, [SendPrompt, Interrupt, Quit]);

pub fn run(config: Config) -> Result<(), String> {
    let workspace = config.workspace.display().to_string();
    let controller = Controller::spawn(config)?;
    Application::new().run(move |cx: &mut App| {
        text_input::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-enter", SendPrompt, Some("Agent")),
            KeyBinding::new("cmd-enter", SendPrompt, Some("Agent")),
            KeyBinding::new("escape", Interrupt, Some("Agent")),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx| {
            cx.quit();
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1180.), px(820.)), cx);
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(760.), px(560.))),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let input = cx.new(TextInput::new);
                    window.focus(&input.focus_handle(cx));
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(50))
                                .await;
                            if this
                                .update(cx, |view: &mut AgentView, cx| {
                                    let mut changed = false;
                                    // Bound each tick so a noisy command cannot starve input.
                                    for _ in 0..512 {
                                        let Ok(event) = view.controller.events.try_recv() else {
                                            break;
                                        };
                                        view.state.apply(event);
                                        changed = true;
                                    }
                                    view.decisions_sent.retain(|id| {
                                        view.state.approvals.iter().any(|a| &a.id == id)
                                    });
                                    if changed {
                                        cx.notify();
                                    }
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();
                    AgentView {
                        controller,
                        state: AppState::default(),
                        input,
                        workspace,
                        diagnostics: false,
                        decisions_sent: Vec::new(),
                        transcript_scroll: ScrollHandle::new(),
                    }
                })
            },
        ) {
            Ok(_) => cx.activate(true),
            Err(error) => {
                eprintln!("Could not open GPUI window: {error}");
                cx.quit();
            }
        }
    });
    Ok(())
}

struct AgentView {
    controller: Controller,
    state: AppState,
    input: Entity<TextInput>,
    workspace: String,
    diagnostics: bool,
    decisions_sent: Vec<crate::protocol::RequestId>,
    transcript_scroll: ScrollHandle,
}

impl Drop for AgentView {
    fn drop(&mut self) {
        let _ = self.controller.send(Command::Shutdown);
    }
}

fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .text_sm()
        .bg(if active { rgb(0x234d43) } else { rgb(0x252b34) })
        .text_color(if active { rgb(0xd6f6e6) } else { rgb(0x7c8798) })
        .when(active, |d| {
            d.cursor_pointer().hover(|s| s.bg(rgb(0x376459)))
        })
        .child(label.into())
}

impl AgentView {
    fn decide(&mut self, id: crate::protocol::RequestId, accept: bool, cx: &mut Context<Self>) {
        if self.decisions_sent.contains(&id) {
            return;
        }
        let Some(approval) = self.state.approvals.iter().find(|a| a.id == id) else {
            return;
        };
        if !crate::protocol::allows_decision(&approval.params, accept) {
            return;
        }
        match self.controller.send(Command::Decide {
            id: id.clone(),
            accept,
        }) {
            Ok(()) => self.decisions_sent.push(id),
            Err(error) => self.state.last_error = Some(error),
        }
        cx.notify();
    }
    fn can_send(&self) -> bool {
        self.state.connected && !self.state.busy && self.state.approvals.is_empty()
    }
    fn send(&mut self, _: &SendPrompt, _: &mut Window, cx: &mut Context<Self>) {
        if !self.can_send() || self.input.read(cx).is_composing() {
            return;
        }
        let prompt = self.input.read(cx).text();
        if prompt.trim().is_empty() {
            return;
        }
        if prompt.len() > 1024 * 1024 {
            self.state.last_error = Some("Prompt exceeds 1 MiB limit".into());
            cx.notify();
            return;
        }
        match self.controller.send(Command::SendPrompt(prompt)) {
            Ok(()) => {
                // Close the double-click race before the worker's first event arrives.
                self.state.busy = true;
                self.state.status = "Starting turn…".into();
                self.input.update(cx, |input, cx| {
                    input.reset();
                    cx.notify();
                });
                self.transcript_scroll.scroll_to_bottom();
            }
            Err(error) => self.state.last_error = Some(error),
        }
        cx.notify();
    }
    fn interrupt(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.busy {
            if let Err(error) = self.controller.send(Command::Interrupt) {
                self.state.last_error = Some(error);
            }
            cx.notify();
        }
    }
}

impl Render for AgentView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_send = self.can_send();
        let busy = self.state.busy;
        div().size_full().flex().flex_col().bg(rgb(0x11161d)).text_color(rgb(0xe3e8ef))
            .font_family("sans-serif").text_sm().key_context("Agent")
            .on_action(cx.listener(Self::send)).on_action(cx.listener(Self::interrupt))
            .child(div().px_5().py_4().border_b_1().border_color(rgb(0x2a3442))
                .child(div().text_xl().child("Coding Agent"))
                .child(div().text_color(rgb(0x9ba9bd)).child(self.workspace.clone()))
                .child(div().mt_2().flex().gap_3().items_center()
                    .child(format!("{} · {}", if self.state.connected { "Connected" } else { "Not connected" }, self.state.status))
                    .child(button("diagnostics", if self.diagnostics { "Hide diagnostics" } else { "Diagnostics" }, true)
                        .on_click(cx.listener(|view, _, _, cx| { view.diagnostics = !view.diagnostics; cx.notify(); })))
                    .when_some(self.state.thread_id.clone(), |d, id| d.child(button("copy-thread", "Copy thread ID", true)
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(id.clone())))))))
            .when_some(self.state.last_error.clone(), |d, error| d.child(div().px_5().py_2().bg(rgb(0x502932)).child(error)))
            .child(div().flex().flex_1().min_h_0().gap_3().p_3()
                .child(div().flex().flex_col().flex_1().min_w_0().gap_2()
                    .child(div().flex().justify_between().items_center().child("Conversation")
                        .child(button("latest", "Jump to latest", true).on_click(cx.listener(|view, _, _, cx| {
                            view.transcript_scroll.scroll_to_bottom(); cx.notify();
                        }))))
                    .child(div().id("transcript").overflow_y_scroll().track_scroll(&self.transcript_scroll)
                        .flex_1().min_h_0().p_3().bg(rgb(0x18202a)).rounded_md()
                        .when(self.state.items.is_empty(), |d| d.child("Send a task to begin. Commands and edits requiring approval appear below."))
                        .children(self.state.items.iter().enumerate().map(|(i, item)| {
                            div().id(("item", i)).mb_4().p_3().rounded_md().bg(rgb(0x202b38))
                                .child(div().mb_2().text_color(rgb(0x9dd6bf)).child(format!("{} · {}", item.kind, item.status)))
                                .child(div().whitespace_normal().child(item.text.clone()))
                        }))))
                .child(div().w(px(350.)).flex().flex_col().gap_2()
                    .child("Latest unified diff")
                    .child(div().id("diff").flex_1().min_h_0().overflow_scroll().p_3().rounded_md().bg(rgb(0x18202a))
                        .font_family("monospace").text_xs()
                        .children(if self.state.diff.is_empty() { vec!["No diff received yet.".to_owned()] } else {
                            self.state.diff.lines().map(str::to_owned).collect()
                        }.into_iter().map(|line| {
                            let color = if line.starts_with('+') { 0xa0dfb0 } else if line.starts_with('-') { 0xf2aaa9 } else { 0xbac5d4 };
                            div().whitespace_nowrap().text_color(rgb(color)).child(if line.is_empty() { " ".into() } else { line })
                        })))))
            .when(!self.state.approvals.is_empty(), |d| d.child(
                div().id("approvals").max_h(px(250.)).overflow_y_scroll().px_4().py_2().bg(rgb(0x3a3020))
                    .children(self.state.approvals.iter().enumerate().map(|(i, approval)| {
                        let accept_id = approval.id.clone();
                        let reject_id = approval.id.clone();
                        let pending = self.decisions_sent.contains(&approval.id);
                        let accept_allowed = crate::protocol::allows_decision(&approval.params, true);
                        let decline_allowed = crate::protocol::allows_decision(&approval.params, false);
                        div().mb_3().child(format!("Approval required: {}", approval.method))
                            .when(!accept_allowed && !decline_allowed, |d| d.child("This request offers no supported one-time decision. Interrupt the turn to cancel."))
                            .child(div().id(("approval-details", i)).max_h(px(130.)).overflow_scroll().font_family("monospace").text_xs()
                                .children(serde_json::to_string_pretty(&approval.params).unwrap_or_default().lines().map(|line| div().whitespace_nowrap().child(line.to_owned())).collect::<Vec<_>>()))
                            .child(div().flex().gap_2().mt_2()
                                .child(button(("accept", i), if pending { "Decision sent…" } else { "Accept once" }, !pending && accept_allowed).on_click(cx.listener(move |view, _, _, cx| {
                                    view.decide(accept_id.clone(), true, cx);
                                })))
                                .child(button(("decline", i), "Decline", !pending && decline_allowed).on_click(cx.listener(move |view, _, _, cx| {
                                    view.decide(reject_id.clone(), false, cx);
                                }))))
                    }))))
            .when(self.diagnostics, |d| d.child(div().id("logs").h(px(130.)).overflow_scroll().p_3().bg(rgb(0x090d12)).font_family("monospace").text_xs()
                .child(format!("Thread: {}", self.state.thread_id.as_deref().unwrap_or("not created")))
                .children(self.state.logs.iter().rev().take(100).rev().map(|line| div().child(line.clone())))))
            .child(div().px_4().py_3().border_t_1().border_color(rgb(0x2a3442)).flex().flex_col().gap_2()
                .child(self.input.clone())
                .child(div().flex().justify_between().items_center()
                    .child(div().text_color(rgb(0x9ba9bd)).child("Ctrl/⌘ + Enter to send · Esc to interrupt · Single-line composer"))
                    .child(div().flex().gap_2()
                        .child(button("interrupt", "Interrupt", busy).on_click(cx.listener(|v, _, w, cx| v.interrupt(&Interrupt, w, cx))))
                        .child(button("send", "Send task", can_send).on_click(cx.listener(|v, _, w, cx| v.send(&SendPrompt, w, cx)))))))
    }
}
