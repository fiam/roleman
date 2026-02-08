use std::fs::OpenOptions;

#[path = "../common/mod.rs"]
mod common;

use common::{MockServerOptions, run_mock_server};
use tracing_subscriber::prelude::*;

fn main() {
    let env_filter = tracing_subscriber::EnvFilter::from_default_env();
    let log_file = std::env::var("ROLEMAN_LOG_FILE").ok();
    let _guard = if let Some(path) = log_file {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("failed to open ROLEMAN_LOG_FILE");
        let (writer, guard) = tracing_appender::non_blocking(file);
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(tracing_subscriber::fmt::layer().with_writer(writer))
            .init();
        Some(guard)
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        None
    };

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut options = MockServerOptions::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--port" => {
                index += 1;
                let value = args.get(index).cloned().unwrap_or_default();
                let parsed = value.parse::<u16>().ok();
                if parsed.is_none() {
                    exit_usage("invalid value for --port");
                }
                options.port = parsed.unwrap_or(options.port);
            }
            "-h" | "--help" => {
                print_usage();
                return;
            }
            _ => {
                exit_usage("unexpected argument");
            }
        }
        index += 1;
    }

    let runtime = tokio::runtime::Runtime::new().expect("failed to start runtime");
    if let Err(err) = runtime.block_on(run_mock_server(options)) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }

    drop(_guard);
}

fn print_usage() {
    eprintln!("usage: roleman-mock-server [--port <port>]");
}

fn exit_usage(message: &str) -> ! {
    eprintln!("error: {message}");
    print_usage();
    std::process::exit(2);
}
