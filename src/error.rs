use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("missing AWS SSO cache for start URL")]
    MissingCache,
    #[error("SSO cache is expired for start URL")]
    ExpiredCache,
    #[error("failed to parse cache file: {path}")]
    CacheParse { path: PathBuf },
    #[error("aws sdk error: {0}")]
    AwsSdk(String),
    #[error("tui error: {0}")]
    Tui(String),
    #[error("no role selection was made")]
    NoSelection,
    #[error("HOME is not set")]
    MissingHome,
    #[error("missing SSO start URL (pass it or set in config)")]
    MissingStartUrl,
    #[error("missing SSO region (pass it or set in config)")]
    MissingRegion,
    #[error("missing SSO account (configure accounts or pass --account)")]
    MissingAccount,
    #[error("failed to open browser: {0}")]
    OpenBrowser(String),
    #[error("config error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;
