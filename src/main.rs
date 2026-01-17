use std::path::PathBuf;

use roleman::{App, AppOptions};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut options = AppOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sso-start-url" => {
                options.start_url = args.next();
                if options.start_url.is_none() {
                    exit_usage("missing value for --sso-start-url");
                }
            }
            "--manage-hidden" => {
                options.manage_hidden = true;
            }
            "--sso-region" => {
                options.sso_region = args.next();
                if options.sso_region.is_none() {
                    exit_usage("missing value for --sso-region");
                }
            }
            "--refresh-seconds" => {
                let value = args.next().unwrap_or_default();
                let parsed = value.parse::<u64>().ok();
                if parsed.is_none() {
                    exit_usage("invalid value for --refresh-seconds");
                }
                options.refresh_seconds = parsed;
            }
            "--config" => {
                let value = args.next().unwrap_or_default();
                if value.is_empty() {
                    exit_usage("missing value for --config");
                }
                options.config_path = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                print_usage();
                return;
            }
            _ => {
                if options.start_url.is_none() {
                    options.start_url = Some(arg);
                } else {
                    exit_usage("unexpected argument");
                }
            }
        }
    }

    if let Err(err) = App::new(options).run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!(
        "usage: roleman [--sso-start-url <url>] [--sso-region <region>] [--manage-hidden] [--refresh-seconds <n>] [--config <path>]\n       roleman <sso-start-url>"
    );
}

fn exit_usage(message: &str) -> ! {
    eprintln!("error: {message}");
    print_usage();
    std::process::exit(2);
}
