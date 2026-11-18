use gpui_telegram_cli::run_cli_from_env;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    match run_cli_from_env().await {
        Ok(output) => {
            if !output.is_empty() {
                if output.ends_with('\n') {
                    print!("{output}");
                } else {
                    println!("{output}");
                }
            }
        }
        Err(err) => {
            let err_json =
                serde_json::to_string_pretty(&err.to_json()).unwrap_or_else(|_| err.to_string());
            eprintln!("{err_json}");
            std::process::exit(err.exit_code());
        }
    }
}
