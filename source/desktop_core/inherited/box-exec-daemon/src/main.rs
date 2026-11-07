use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender};
use std::thread;

use serde_json::json;

use crate::server::{BoxExecDaemonOptions, start_box_exec_daemon};

pub fn run_box_exec_daemon_entrypoint() -> Result<(), String> {
    let workspace_root = match env::var_os("SAND_BOX_WORKSPACE_ROOT") {
        Some(value) => PathBuf::from(value),
        None => env::current_dir().map_err(|error| error.to_string())?,
    };
    let port = match env::var("SAND_BOX_EXEC_DAEMON_PORT") {
        Ok(raw) => Some(
            raw.trim()
                .parse::<u16>()
                .map_err(|_| format!("invalid SAND_BOX_EXEC_DAEMON_PORT: {raw}"))?,
        ),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(error.to_string()),
    };
    let terminals_directory =
        env::var_os("SAND_BOX_TERMINALS_DIRECTORY").map(PathBuf::from);
    let auth_token = env::var("SAND_BOX_EXEC_DAEMON_AUTH_TOKEN").ok();

    let handle = start_box_exec_daemon(BoxExecDaemonOptions {
        host: None,
        port,
        auth_token,
        workspace_root,
        terminals_directory,
        environment: None,
    })?;

    println!(
        "{}",
        json!({
            "event": "box-exec-daemon-ready",
            "url": handle.url,
            "workspaceRoot": handle.workspace_root,
            "terminalsDirectory": handle.terminals_directory,
        })
    );

    let (shutdown_tx, shutdown_rx) = mpsc::sync_channel::<()>(2);
    let signal_shutdown_tx = shutdown_tx.clone();
    ctrlc::set_handler(move || {
        let _ = signal_shutdown_tx.try_send(());
    })
    .map_err(|error| format!("could not install box-exec daemon signal handler: {error}"))?;

    if env_flag("SAND_BOX_PARENT_PIPE") {
        spawn_parent_pipe_watcher(shutdown_tx);
    }

    let _ = shutdown_rx.recv();
    handle.stop()?;
    Ok(())
}


fn spawn_parent_pipe_watcher(shutdown_tx: SyncSender<()>) {
    let _ = thread::Builder::new()
        .name("box-exec-daemon-parent-pipe".into())
        .spawn(move || {
            let mut stdin = std::io::stdin();
            signal_shutdown_on_eof(&mut stdin, &shutdown_tx);
        });
}

fn signal_shutdown_on_eof<R: Read>(reader: &mut R, shutdown_tx: &SyncSender<()>) {
    let mut buffer = [0_u8; 1];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => {
                let _ = shutdown_tx.try_send(());
                break;
            }
            Ok(_) => {}
        }
    }
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::signal_shutdown_on_eof;

    #[test]
    fn parent_pipe_eof_requests_shutdown() {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut reader = Cursor::new(Vec::<u8>::new());
        signal_shutdown_on_eof(&mut reader, &tx);
        rx.recv_timeout(Duration::from_secs(1))
            .expect("parent EOF shutdown");
    }
}
