mod text_input;
mod ui;
// Keep the UI module's core imports identical to the library API.
use coding_agent_desktop::{engine, protocol, state};
fn main() {
    let mut config = engine::Config::new(std::env::current_dir().expect("current directory"));
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--workspace") => {
                config.workspace = args.next().expect("--workspace requires a path").into()
            }
            Some("--codex") => {
                config.executable = args.next().expect("--codex requires an executable").into()
            }
            Some("--model") => {
                config.model = Some(
                    args.next()
                        .expect("--model requires a model ID")
                        .to_string_lossy()
                        .into(),
                )
            }
            Some("--resume") => {
                config.resume = Some(
                    args.next()
                        .expect("--resume requires a thread ID")
                        .to_string_lossy()
                        .into(),
                )
            }
            Some("--read-only") => config.read_only = true,
            Some("--help" | "-h") => {
                println!(
                    "coding-agent-desktop [--workspace PATH] [--codex PATH] [--model ID] [--resume THREAD_ID] [--read-only]\n\nUses Codex CLI credentials/config. Default: workspace-write sandbox, on-request approvals, human reviewer. No automatic approval or retry. This is a single-thread MVP."
                );
                return;
            }
            _ => {
                eprintln!("Unknown argument: {}. Use --help.", arg.to_string_lossy());
                std::process::exit(2);
            }
        }
    }
    if let Err(error) = ui::run(config) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
