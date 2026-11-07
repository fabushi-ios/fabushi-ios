use fabushi_box_exec_daemon::entrypoint::run_box_exec_daemon_entrypoint;

fn main() {
    if let Err(error) = run_box_exec_daemon_entrypoint() {
        eprintln!("box-exec-daemon: {error}");
        std::process::exit(1);
    }
}
