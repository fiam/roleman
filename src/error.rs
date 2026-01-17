use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("missing AWS SSO cache for start URL")]
    MissingCache,
    #[error("SSO cache is expired for start URL")]
    ExpiredCache,
    #[error("failed to parse cache file: {path}")]
    CacheParse { path: PathBuf },
    #[error("failed to execute aws CLI: {0}")]
    AwsCli(String),
    #[error("unexpected aws CLI output: {0}")]
    AwsCliOutput(String),
    #[error("no role selection was made")]
    NoSelection,
    #[error("HOME is not set")]
    MissingHome,
    #[error("missing SSO start URL (pass it or set in config)")]
    MissingStartUrl,
    #[error("missing SSO region (pass it or set in config)")]
    MissingRegion,
    #[error("config error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;
