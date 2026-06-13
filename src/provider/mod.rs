//! Cloud provider abstraction.
//!
//! `roleman` selects a credential target (account + role) and exports temporary
//! credentials into the shell. Everything provider-specific — how to authenticate,
//! how to list targets, how to mint and render credentials, how to drop privileges —
//! lives behind [`CloudProvider`] so the orchestration in `lib.rs` stays generic.
//!
//! AWS is implemented in [`aws`]. A GCP implementation (service-account impersonation
//! with Credential Access Boundaries for `--readonly`) can be added as a sibling module
//! without touching the generic layer.

pub mod aws;

use std::any::Any;

use crate::config::{ProviderKind, SsoIdentity};
use crate::error::{Error, Result};
use crate::model::RoleChoice;

pub use aws::cli::PostLoginActions;

/// The privilege level to mint credentials at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessScope {
    /// Base role credentials, unchanged.
    #[default]
    Full,
    /// Drop write access (`--readonly`).
    ReadOnly,
}

impl AccessScope {
    /// Stable tag used to key cached credentials so scoped creds cache separately.
    pub fn cache_tag(self) -> &'static str {
        match self {
            AccessScope::Full => "full",
            AccessScope::ReadOnly => "readonly",
        }
    }

    /// Suffix appended to profile names so a scoped identity is distinct.
    pub fn profile_suffix(self) -> &'static str {
        match self {
            AccessScope::Full => "",
            AccessScope::ReadOnly => "@readonly",
        }
    }
}

/// Whether a choice is the currently-active credential target, for the selector marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMarker {
    /// Not the active target.
    Inactive,
    /// Active target with valid cached credentials.
    ActiveValid,
    /// Active target whose cached credentials are missing or expired.
    ActiveStale,
}

/// The provider-side profile binding for a selected target (AWS: a `~/.aws/config` profile).
pub struct ProfileBinding {
    pub profile_name: String,
    pub config_file: Option<String>,
}

/// Opaque, provider-owned session/token state threaded back into later calls.
pub trait ProviderSession: Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

/// A single environment variable to export into the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

impl EnvVar {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Provider-specific credentials that know how to render and cache themselves.
pub trait ProviderCredentials: Send + Sync {
    /// Ordered environment variables to export (AWS: `AWS_*`; GCP: `CLOUDSDK_*`/`GOOGLE_*`).
    fn env_vars(&self, binding: &ProfileBinding) -> Vec<EnvVar>;
    /// Expiration in unix-ms, for cache-expiry checks.
    fn expiration_ms(&self) -> u64;
    /// Serialize for the on-disk credentials cache.
    fn to_cache_json(&self) -> Result<String>;
}

/// Render environment variables as shell `export NAME=value` lines.
pub fn export_lines(vars: &[EnvVar]) -> String {
    vars.iter()
        .map(|var| format!("export {}={}", var.name, var.value))
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait::async_trait]
pub trait CloudProvider: Send + Sync {
    /// Ensure a valid auth token exists (load cache or trigger interactive login).
    async fn ensure_session(
        &self,
        ignore_cache: bool,
        post_login: PostLoginActions,
    ) -> Result<Box<dyn ProviderSession>>;

    /// List selectable account/role targets for this session.
    async fn list_choices(&self, session: &dyn ProviderSession) -> Result<Vec<RoleChoice>>;

    /// Mint credentials for a selected target at the requested scope.
    ///
    /// `may_create` authorizes the provider to create cloud resources it needs (e.g. an AWS
    /// read-only role for [`AccessScope::ReadOnly`]). When `false` and a resource is missing,
    /// the provider returns [`Error::NeedsResourceCreation`] instead of creating it, so the
    /// caller can obtain consent and retry.
    async fn fetch_credentials(
        &self,
        session: &dyn ProviderSession,
        choice: &RoleChoice,
        scope: AccessScope,
        may_create: bool,
    ) -> Result<Box<dyn ProviderCredentials>>;

    /// Reconstruct credentials from a cached payload produced by [`ProviderCredentials::to_cache_json`].
    fn credentials_from_cache_json(&self, json: &str) -> Result<Box<dyn ProviderCredentials>>;

    /// Persist any provider-side profile config and return the binding to export.
    fn ensure_profile(
        &self,
        session: &dyn ProviderSession,
        choice: &RoleChoice,
        scope: AccessScope,
        omit_role_name: bool,
    ) -> Result<ProfileBinding>;

    /// Build the web/console URL for the Open action.
    fn console_url(&self, choice: &RoleChoice) -> String;

    /// Provider-agnostic cache namespace for this identity (identity name + provider).
    fn cache_namespace(&self) -> String;

    /// Resolve the selector active-marker for each choice, aligned to the input slice.
    fn active_markers(&self, choices: &[RoleChoice], scope: AccessScope) -> Vec<ActiveMarker>;

    /// Whether this provider can enforce the requested scope.
    fn supports_scope(&self, _scope: AccessScope) -> bool {
        true
    }

    /// Account identifier cleanup is operating on (from ambient credentials), for display.
    async fn current_account(&self) -> Result<String> {
        Err(Error::Config(
            "this provider does not support resource cleanup".to_string(),
        ))
    }

    /// List roleman-created resources in a single account, minting fresh credentials via the
    /// SSO session (so cleanup works even from a read-only shell). Defaults to none for
    /// providers that don't create anything.
    async fn list_managed_resources_in(
        &self,
        _session: &dyn ProviderSession,
        _account_id: &str,
    ) -> Result<Vec<ManagedResource>> {
        Ok(Vec::new())
    }

    /// List roleman-created resources across *every* account reachable from the SSO session.
    ///
    /// Mints credentials per account, so it is slower and noisier than the single-account
    /// path. Accounts where roleman has no IAM access are reported via
    /// [`ManagedResource`]-less [`AccountCleanup`] entries rather than failing the whole sweep.
    /// `progress` is called with a human-readable status as each account is checked, so the
    /// caller can show liveness during the (slow) sweep.
    async fn list_managed_resources_all(
        &self,
        _session: &dyn ProviderSession,
        _progress: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Vec<AccountCleanup>> {
        Err(Error::Config(
            "this provider does not support resource cleanup".to_string(),
        ))
    }

    /// Delete a resource in a specific account, minting credentials for it via the session.
    async fn delete_managed_resource_in(
        &self,
        _session: &dyn ProviderSession,
        _resource: &ManagedResource,
    ) -> Result<()> {
        Err(Error::Config(
            "this provider does not support resource cleanup".to_string(),
        ))
    }
}

/// A cloud resource roleman created and is responsible for cleaning up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResource {
    /// Resource kind, e.g. `iam-role`.
    pub kind: String,
    /// Stable identifier used for deletion (e.g. the role name).
    pub id: String,
    /// Fully-qualified ARN/URI, for display.
    pub arn: String,
    /// Human-readable detail (e.g. attached policies), for the cleanup listing.
    pub detail: String,
    /// Account the resource lives in (needed to mint deletion credentials during `--all`).
    pub account_id: String,
}

/// Per-account result of an `--all` cleanup scan.
#[derive(Debug, Clone)]
pub struct AccountCleanup {
    pub account_id: String,
    pub account_name: String,
    /// Resources found in the account (empty when none or when inaccessible).
    pub resources: Vec<ManagedResource>,
    /// Why the account couldn't be scanned (e.g. no IAM access), if applicable.
    pub error: Option<String>,
}

/// Construct the provider for an identity based on its configured [`ProviderKind`].
pub fn for_identity(identity: &SsoIdentity) -> Result<Box<dyn CloudProvider>> {
    match identity.provider {
        ProviderKind::Aws => Ok(Box::new(aws::AwsProvider::new(identity.clone()))),
        ProviderKind::Gcp => Err(Error::Config(
            "GCP provider is not implemented yet".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(provider: ProviderKind) -> SsoIdentity {
        SsoIdentity {
            name: "work".into(),
            start_url: "https://acme.awsapps.com/start".into(),
            sso_region: "us-east-1".into(),
            provider,
            readonly_policy: None,
            accounts: Vec::new(),
            ignore_roles: Vec::new(),
        }
    }

    #[test]
    fn for_identity_dispatches_aws() {
        assert!(for_identity(&identity(ProviderKind::Aws)).is_ok());
    }

    #[test]
    fn for_identity_rejects_unimplemented_gcp() {
        assert!(for_identity(&identity(ProviderKind::Gcp)).is_err());
    }

    // A minimal non-AWS provider proving the generic layer is provider-agnostic:
    // the trait is object-safe and the credential/scope flow composes without AWS.
    struct FakeSession;
    impl ProviderSession for FakeSession {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct FakeCreds {
        scope: AccessScope,
    }
    impl ProviderCredentials for FakeCreds {
        fn env_vars(&self, binding: &ProfileBinding) -> Vec<EnvVar> {
            vec![
                EnvVar::new("FAKE_TOKEN", self.scope.cache_tag()),
                EnvVar::new("FAKE_PROFILE", binding.profile_name.clone()),
            ]
        }
        fn expiration_ms(&self) -> u64 {
            0
        }
        fn to_cache_json(&self) -> Result<String> {
            Ok(self.scope.cache_tag().to_string())
        }
    }

    struct FakeProvider;
    #[async_trait::async_trait]
    impl CloudProvider for FakeProvider {
        async fn ensure_session(
            &self,
            _ignore_cache: bool,
            _post_login: PostLoginActions,
        ) -> Result<Box<dyn ProviderSession>> {
            Ok(Box::new(FakeSession))
        }
        async fn list_choices(&self, _session: &dyn ProviderSession) -> Result<Vec<RoleChoice>> {
            Ok(vec![RoleChoice {
                account_id: "1".into(),
                account_name: "Acme".into(),
                role_name: "Admin".into(),
            }])
        }
        async fn fetch_credentials(
            &self,
            _session: &dyn ProviderSession,
            _choice: &RoleChoice,
            scope: AccessScope,
            may_create: bool,
        ) -> Result<Box<dyn ProviderCredentials>> {
            // Mirror the AWS provider: ReadOnly needs a resource that requires consent.
            if matches!(scope, AccessScope::ReadOnly) && !may_create {
                return Err(Error::NeedsResourceCreation("fake resource".into()));
            }
            Ok(Box::new(FakeCreds { scope }))
        }
        fn credentials_from_cache_json(&self, _json: &str) -> Result<Box<dyn ProviderCredentials>> {
            Ok(Box::new(FakeCreds {
                scope: AccessScope::Full,
            }))
        }
        fn ensure_profile(
            &self,
            _session: &dyn ProviderSession,
            _choice: &RoleChoice,
            scope: AccessScope,
            _omit_role_name: bool,
        ) -> Result<ProfileBinding> {
            Ok(ProfileBinding {
                profile_name: format!("acme{}", scope.profile_suffix()),
                config_file: None,
            })
        }
        fn console_url(&self, _choice: &RoleChoice) -> String {
            "https://example.test".into()
        }
        fn cache_namespace(&self) -> String {
            "fake".into()
        }
        fn active_markers(&self, choices: &[RoleChoice], _scope: AccessScope) -> Vec<ActiveMarker> {
            vec![ActiveMarker::Inactive; choices.len()]
        }
    }

    #[tokio::test]
    async fn fake_provider_threads_scope_through_the_trait() {
        let provider: Box<dyn CloudProvider> = Box::new(FakeProvider);
        let session = provider
            .ensure_session(false, PostLoginActions::default())
            .await
            .unwrap();
        let choices = provider.list_choices(session.as_ref()).await.unwrap();
        assert_eq!(choices.len(), 1);

        // Without consent, ReadOnly asks the caller to authorize resource creation.
        let needs = provider
            .fetch_credentials(session.as_ref(), &choices[0], AccessScope::ReadOnly, false)
            .await;
        assert!(matches!(needs, Err(Error::NeedsResourceCreation(_))));

        // With consent (may_create=true), it succeeds.
        let creds = provider
            .fetch_credentials(session.as_ref(), &choices[0], AccessScope::ReadOnly, true)
            .await
            .unwrap();
        let binding = provider
            .ensure_profile(session.as_ref(), &choices[0], AccessScope::ReadOnly, true)
            .unwrap();
        let vars = creds.env_vars(&binding);
        // The ReadOnly scope flows end-to-end without the generic layer knowing the provider.
        assert!(vars.contains(&EnvVar::new("FAKE_TOKEN", "readonly")));
        assert!(vars.contains(&EnvVar::new("FAKE_PROFILE", "acme@readonly")));
        // The generic formatter turns pairs into shell exports.
        let lines = export_lines(&vars);
        assert!(lines.contains("export FAKE_TOKEN=readonly"));
    }
}
