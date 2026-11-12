//! Handshake + thread smoke test. Never sends a prompt or invokes a model.
use coding_agent_desktop::{
    engine::{Command, Config, Controller},
    state::Event,
};
fn run() -> Result<(), String> {
    let mut config = Config::new(std::env::current_dir().map_err(|e| e.to_string())?);
    if let Some(binary) = std::env::args_os().nth(1) {
        config.executable = binary.into();
    }
    config.read_only = true;
    let controller = Controller::spawn(config)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(55);
    loop {
        while let Ok(event) = controller.events.try_recv() {
            match event {
                Event::ThreadOpened { thread, .. } => {
                    println!("PASS: initialized; thread opened: {}", thread["id"]);
                    let _ = controller.send(Command::Shutdown);
                    return Ok(());
                }
                Event::Error(error) | Event::Disconnected(error) => return Err(error),
                Event::Connected => println!("PASS: initialize / initialized"),
                Event::Log(log) => eprintln!("{log}"),
                _ => {}
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
fn main() {
    if let Err(error) = run() {
        // Controller has already dropped, allowing graceful child shutdown before exit.
        eprintln!("FAIL: {error}");
        std::process::exit(1);
    }
}
