//! AWS IAM Identity Center (SSO) implementation of [`CloudProvider`].

pub mod cli;
pub mod config;
pub mod sdk;
pub mod sso_cache;

use std::any::Any;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::debug;

use self::cli as aws_cli;
use self::config as aws_config;
use self::sdk as aws_sdk;
use crate::config::{ReadonlyPolicy, SsoIdentity};
use crate::credentials_cache::{self, CachedCredentialsStatus};
use crate::error::{Error, Result};
use crate::model::{AwsRoleCredentials, CacheEntry, RoleChoice};
use crate::provider::{
    AccessScope, AccountCleanup, ActiveMarker, CloudProvider, EnvVar, ManagedResource,
    PostLoginActions, ProfileBinding, ProviderCredentials, ProviderSession,
};
use crate::ui;
use sha1::{Digest, Sha1};

/// IAM path that namespaces roleman-created roles, for easy discovery/cleanup.
const READONLY_ROLE_PATH: &str = "/roleman/";
/// Inline policy name used when `readonly_policy` is an inline document.
const READONLY_INLINE_POLICY_NAME: &str = "roleman-readonly";
const MANAGED_BY_TAG_KEY: &str = "ManagedBy";
const MANAGED_BY_TAG_VALUE: &str = "roleman";
const PURPOSE_TAG_KEY: &str = "roleman:purpose";
const PURPOSE_TAG_VALUE: &str = "readonly-downscope";
/// Tag recording which caller (SSO user) a read-only role belongs to.
const OWNER_TAG_KEY: &str = "roleman:owner";
/// Tag recording the full original caller identity (ARN) that created the role.
const CREATED_BY_TAG_KEY: &str = "roleman:created-by";

/// AWS provider bound to a single configured identity.
pub struct AwsProvider {
    identity: SsoIdentity,
}

impl AwsProvider {
    pub fn new(identity: SsoIdentity) -> Self {
        Self { identity }
    }
}

/// AWS session state: the SSO access token and the region it was issued in.
pub struct AwsSession {
    cache: CacheEntry,
}

impl ProviderSession for AwsSession {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn session_of(session: &dyn ProviderSession) -> Result<&AwsSession> {
    session
        .as_any()
        .downcast_ref::<AwsSession>()
        .ok_or_else(|| Error::AwsSdk("session/provider mismatch".into()))
}

/// AWS credentials ready for export, including the region they target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    expiration_ms: u64,
    region: String,
}

impl AwsCredentials {
    fn from_raw(raw: AwsRoleCredentials, region: &str) -> Self {
        Self {
            access_key_id: raw.access_key_id,
            secret_access_key: raw.secret_access_key,
            session_token: raw.session_token,
            expiration_ms: raw.expiration,
            region: region.to_string(),
        }
    }
}

impl ProviderCredentials for AwsCredentials {
    fn env_vars(&self, binding: &ProfileBinding) -> Vec<EnvVar> {
        let mut vars = vec![
            EnvVar::new("AWS_ACCESS_KEY_ID", self.access_key_id.clone()),
            EnvVar::new("AWS_SECRET_ACCESS_KEY", self.secret_access_key.clone()),
            EnvVar::new("AWS_SESSION_TOKEN", self.session_token.clone()),
            EnvVar::new(
                "AWS_CREDENTIAL_EXPIRATION",
                format_expiration(self.expiration_ms),
            ),
            EnvVar::new("AWS_DEFAULT_REGION", self.region.clone()),
            EnvVar::new("AWS_REGION", self.region.clone()),
            EnvVar::new("AWS_PROFILE", binding.profile_name.clone()),
        ];
        if let Some(path) = &binding.config_file {
            vars.push(EnvVar::new("AWS_CONFIG_FILE", path.clone()));
        }
        vars
    }

    fn expiration_ms(&self) -> u64 {
        self.expiration_ms
    }

    fn to_cache_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|err| Error::AwsSdk(err.to_string()))
    }
}

#[async_trait::async_trait]
impl CloudProvider for AwsProvider {
    async fn ensure_session(
        &self,
        ignore_cache: bool,
        post_login: PostLoginActions,
    ) -> Result<Box<dyn ProviderSession>> {
        let ignore_sso_cache = env_truthy("ROLEMAN_IGNORE_SSO_CACHE");
        if !ignore_cache
            && !ignore_sso_cache
            && let Ok(entry) = sso_cache::load_valid_cache(&self.identity.start_url)
        {
            return Ok(Box::new(AwsSession { cache: entry }));
        }
        if ignore_sso_cache {
            eprintln!(
                "{}",
                ui::info("Ignoring cached SSO token due to ROLEMAN_IGNORE_SSO_CACHE.")
            );
        }
        let session = aws_config::ensure_sso_session(&self.identity)?;
        aws_cli::sso_login_session(&session, post_login)?;
        let entry = sso_cache::load_valid_cache(&self.identity.start_url)?;
        Ok(Box::new(AwsSession { cache: entry }))
    }

    async fn list_choices(&self, session: &dyn ProviderSession) -> Result<Vec<RoleChoice>> {
        let session = session_of(session)?;
        let token = &session.cache.access_token;
        let region = &session.cache.region;

        let accounts_spinner = ui::spinner("Fetching SSO accounts...");
        let mut accounts = match aws_sdk::list_accounts(token, region).await {
            Ok(accounts) => accounts,
            Err(err) => {
                accounts_spinner.finish_and_clear();
                return Err(err);
            }
        };
        accounts_spinner.finish_with_message(ui::success("Fetched SSO accounts"));
        accounts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        for account in &accounts {
            debug!(account_id = %account.id, account_name = %account.name, "fetched account");
        }

        let roles_spinner = ui::spinner("Fetching roles for all accounts...");
        let roles_by_account = futures::stream::iter(accounts.clone())
            .map(|account| {
                let token = token.clone();
                let region = region.clone();
                async move {
                    let roles = aws_sdk::list_account_roles(&token, &region, &account.id).await?;
                    Ok::<_, Error>((account, roles))
                }
            })
            .buffer_unordered(10)
            .collect::<Vec<_>>()
            .await;

        let roles_by_account = match roles_by_account.into_iter().collect::<Result<Vec<_>>>() {
            Ok(roles) => roles,
            Err(err) => {
                roles_spinner.finish_and_clear();
                return Err(err);
            }
        };
        roles_spinner.finish_with_message(ui::success("Fetched roles"));

        let mut choices = Vec::new();
        for (account, roles) in roles_by_account {
            for role in roles {
                choices.push(RoleChoice::new(&account, &role));
            }
        }
        Ok(choices)
    }

    async fn fetch_credentials(
        &self,
        session: &dyn ProviderSession,
        choice: &RoleChoice,
        scope: AccessScope,
        may_create: bool,
    ) -> Result<Box<dyn ProviderCredentials>> {
        let session = session_of(session)?;
        let region = session.cache.region.clone();
        let base = aws_sdk::get_role_credentials(
            &session.cache.access_token,
            &region,
            &choice.account_id,
            &choice.role_name,
        )
        .await?;

        let creds = match scope {
            AccessScope::Full => AwsCredentials::from_raw(base, &region),
            AccessScope::ReadOnly => {
                self.downscope(&base, &region, &choice.account_id, may_create)
                    .await?
            }
        };
        Ok(Box::new(creds))
    }

    fn credentials_from_cache_json(&self, json: &str) -> Result<Box<dyn ProviderCredentials>> {
        let creds: AwsCredentials =
            serde_json::from_str(json).map_err(|err| Error::AwsSdk(err.to_string()))?;
        Ok(Box::new(creds))
    }

    fn ensure_profile(
        &self,
        session: &dyn ProviderSession,
        choice: &RoleChoice,
        scope: AccessScope,
        omit_role_name: bool,
    ) -> Result<ProfileBinding> {
        let session = session_of(session)?;
        let profile_name = profile_name(choice, scope, omit_role_name);
        aws_config::ensure_role_profile(
            &profile_name,
            choice,
            &self.identity,
            &session.cache.region,
        )?;
        Ok(ProfileBinding {
            profile_name,
            config_file: None,
        })
    }

    fn console_url(&self, choice: &RoleChoice) -> String {
        let base = self.identity.start_url.trim_end_matches('/');
        format!(
            "{base}/#/console?account_id={account_id}&role_name={role}",
            account_id = choice.account_id,
            role = urlencoding::encode(&choice.role_name)
        )
    }

    fn cache_namespace(&self) -> String {
        format!("aws:{}:{}", self.identity.name, self.identity.start_url)
    }

    fn active_markers(&self, choices: &[RoleChoice], scope: AccessScope) -> Vec<ActiveMarker> {
        let namespace = self.cache_namespace();
        let current_profile = std::env::var("AWS_PROFILE").ok();
        let mut roles_per_account: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for choice in choices {
            *roles_per_account
                .entry(choice.account_id.as_str())
                .or_insert(0) += 1;
        }

        choices
            .iter()
            .map(|choice| {
                let Some(active) = current_profile.as_deref() else {
                    return ActiveMarker::Inactive;
                };
                let omit_role_name = roles_per_account
                    .get(choice.account_id.as_str())
                    .copied()
                    .unwrap_or(0)
                    == 1;
                let candidate = profile_name(choice, scope, omit_role_name);
                // Match legacy profile names too, so the marker survives upgrades.
                let legacy = profile_name(choice, scope, false);
                if active != candidate && active != legacy {
                    return ActiveMarker::Inactive;
                }
                match credentials_cache::cached_credentials_status(
                    &namespace,
                    &choice.account_id,
                    &choice.role_name,
                    scope,
                ) {
                    Ok(CachedCredentialsStatus::Valid) => ActiveMarker::ActiveValid,
                    Ok(_) => ActiveMarker::ActiveStale,
                    Err(err) => {
                        debug!(error = %err, "failed to check cached credentials");
                        ActiveMarker::ActiveStale
                    }
                }
            })
            .collect()
    }

    async fn current_account(&self) -> Result<String> {
        aws_sdk::ambient_account(&self.identity.sso_region).await
    }

    async fn list_managed_resources_in(
        &self,
        session: &dyn ProviderSession,
        account_id: &str,
    ) -> Result<Vec<ManagedResource>> {
        let session = session_of(session)?;
        // Mint fresh credentials via SSO (a privileged permission-set role) rather than using
        // whatever is in the shell — which may be a read-only roleman session that can't manage IAM.
        let client = self.account_iam_client(session, account_id).await?;
        collect_managed_roles(&client, account_id).await
    }

    async fn list_managed_resources_all(
        &self,
        session: &dyn ProviderSession,
        progress: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Vec<AccountCleanup>> {
        let session = session_of(session)?;
        let mut accounts =
            aws_sdk::list_accounts(&session.cache.access_token, &session.cache.region).await?;
        accounts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let total = accounts.len();
        let mut results = Vec::new();
        for (index, account) in accounts.into_iter().enumerate() {
            let status = format!(
                "Checking {} ({}) [{}/{}]",
                account.name,
                account.id,
                index + 1,
                total
            );
            progress(&status);
            match self.account_iam_client(session, &account.id).await {
                Ok(client) => {
                    let resources = collect_managed_roles(&client, &account.id)
                        .await
                        .unwrap_or_default();
                    results.push(AccountCleanup {
                        account_id: account.id,
                        account_name: account.name,
                        resources,
                        error: None,
                    });
                }
                Err(err) => results.push(AccountCleanup {
                    account_id: account.id,
                    account_name: account.name,
                    resources: Vec::new(),
                    error: Some(err.to_string()),
                }),
            }
        }
        Ok(results)
    }

    async fn delete_managed_resource_in(
        &self,
        session: &dyn ProviderSession,
        resource: &ManagedResource,
    ) -> Result<()> {
        ensure_iam_role(resource)?;
        let session = session_of(session)?;
        let client = self
            .account_iam_client(session, &resource.account_id)
            .await?;
        delete_role_completely(&client, &resource.id).await
    }
}

impl AwsProvider {
    /// Build an IAM client for an account by minting credentials for one of its roles.
    ///
    /// Tries each role the caller has in the account and returns the first whose credentials
    /// can read IAM (a cheap `ListRoles` probe). Accounts where no role grants IAM access
    /// surface as an error, which the caller records per-account rather than aborting.
    async fn account_iam_client(
        &self,
        session: &AwsSession,
        account_id: &str,
    ) -> Result<aws_sdk_iam::Client> {
        let region = &session.cache.region;
        let roles =
            aws_sdk::list_account_roles(&session.cache.access_token, region, account_id).await?;
        let mut last_err =
            Error::AwsSdk(format!("no role with IAM access in account {account_id}"));
        for role in &roles {
            let base = match aws_sdk::get_role_credentials(
                &session.cache.access_token,
                region,
                account_id,
                &role.name,
            )
            .await
            {
                Ok(base) => base,
                Err(err) => {
                    last_err = err;
                    continue;
                }
            };
            let client = aws_sdk::iam_client_static(region, &base).await?;
            match aws_sdk::list_roles_by_path(&client, READONLY_ROLE_PATH).await {
                Ok(_) => return Ok(client),
                Err(err) => last_err = err,
            }
        }
        Err(last_err)
    }

    /// Drop write access by creating (or reusing) a roleman-owned read-only role and assuming it.
    ///
    /// SSO permission-set roles can't re-assume themselves, so we provision a separate role
    /// that trusts the account's SSO roles and carries a read-only policy, then assume *that*.
    async fn downscope(
        &self,
        base: &AwsRoleCredentials,
        region: &str,
        account_id: &str,
        may_create: bool,
    ) -> Result<AwsCredentials> {
        // Partition + caller identity from the live (base SSO) caller. The caller's session
        // name is the human's identity, so the role name is per-caller: two engineers in the
        // same account get distinct (both read-only) roles.
        let caller_arn = aws_sdk::get_caller_arn(region, base).await.map_err(|err| {
            Error::PermissionDrop(format!("could not resolve current identity: {err}"))
        })?;
        let partition = partition_of(&caller_arn);
        let owner = caller_session_name(&caller_arn).unwrap_or_else(|| caller_arn.clone());
        let role_name = readonly_role_name(&owner);
        let role_arn =
            format!("arn:{partition}:iam::{account_id}:role{READONLY_ROLE_PATH}{role_name}");

        let just_created = self
            .ensure_readonly_role(
                base,
                region,
                account_id,
                &partition,
                &role_name,
                &owner,
                &caller_arn,
                may_create,
            )
            .await?;

        // Always pass a read-only session policy: effective perms = intersection(role, session),
        // so the result is read-only even if the role's attached policy were broader.
        let (policy_arns, inline) = resolve_readonly_policy(&self.identity, &partition);
        let scoped = self
            .assume_with_retry(
                base,
                region,
                &role_arn,
                &policy_arns,
                inline.as_deref(),
                just_created,
            )
            .await?;
        Ok(AwsCredentials::from_raw(scoped, region))
    }

    /// Ensure the per-caller read-only role exists, is roleman-owned, and carries exactly the
    /// desired read-only policy. Returns whether it was just created (so the caller can wait for
    /// IAM propagation before assuming). Returns `NeedsResourceCreation` when it would need to
    /// create the role but `may_create` is false.
    #[allow(clippy::too_many_arguments)]
    async fn ensure_readonly_role(
        &self,
        base: &AwsRoleCredentials,
        region: &str,
        account_id: &str,
        partition: &str,
        role_name: &str,
        owner: &str,
        created_by: &str,
        may_create: bool,
    ) -> Result<bool> {
        let client = aws_sdk::iam_client_static(region, base).await?;
        match aws_sdk::get_role(&client, role_name).await? {
            Some(role) => {
                if !is_roleman_managed(&role.tags) {
                    return Err(Error::PermissionDrop(format!(
                        "a role named {role_name} already exists in {account_id} but is not \
                         managed by roleman (missing {MANAGED_BY_TAG_KEY}={MANAGED_BY_TAG_VALUE} \
                         tag); refusing to assume it"
                    )));
                }
                self.reconcile_readonly_policies(&client, partition, role_name)
                    .await?;
                Ok(false)
            }
            None => {
                if !may_create {
                    return Err(Error::NeedsResourceCreation(format!(
                        "roleman needs to create read-only IAM role `{role_name}` in account \
                         {account_id} to satisfy --readonly"
                    )));
                }
                let trust = readonly_trust_policy(partition, account_id);
                aws_sdk::create_role(
                    &client,
                    role_name,
                    READONLY_ROLE_PATH,
                    &trust,
                    "roleman-managed read-only downscope role",
                    &[
                        (MANAGED_BY_TAG_KEY, MANAGED_BY_TAG_VALUE),
                        (PURPOSE_TAG_KEY, PURPOSE_TAG_VALUE),
                        (OWNER_TAG_KEY, owner),
                        (CREATED_BY_TAG_KEY, created_by),
                    ],
                )
                .await?;
                self.apply_readonly_policies(&client, partition, role_name)
                    .await?;
                // Always announce creation of a cloud-owned resource.
                eprintln!(
                    "{}",
                    ui::action(&format!(
                        "Created IAM role {role_name} in {account_id} \
                         (managed by roleman; remove with `roleman cleanup roles`)"
                    ))
                );
                Ok(true)
            }
        }
    }

    /// Attach the desired read-only policy to a freshly created role.
    async fn apply_readonly_policies(
        &self,
        client: &aws_sdk_iam::Client,
        partition: &str,
        role_name: &str,
    ) -> Result<()> {
        let (arns, inline) = resolve_readonly_policy(&self.identity, partition);
        if let Some(doc) = inline {
            aws_sdk::put_role_policy(client, role_name, READONLY_INLINE_POLICY_NAME, &doc).await?;
        } else {
            for arn in &arns {
                aws_sdk::attach_role_policy(client, role_name, arn).await?;
            }
        }
        Ok(())
    }

    /// Reconcile an existing roleman role so it carries exactly the desired read-only policy
    /// and nothing else (defends against tampering between runs).
    async fn reconcile_readonly_policies(
        &self,
        client: &aws_sdk_iam::Client,
        partition: &str,
        role_name: &str,
    ) -> Result<()> {
        let (desired_arns, inline) = resolve_readonly_policy(&self.identity, partition);
        let attached = aws_sdk::list_attached_role_policies(client, role_name).await?;
        let inline_policies = aws_sdk::list_role_inline_policies(client, role_name).await?;
        if let Some(doc) = inline {
            aws_sdk::put_role_policy(client, role_name, READONLY_INLINE_POLICY_NAME, &doc).await?;
            for arn in &attached {
                aws_sdk::detach_role_policy(client, role_name, arn).await?;
            }
            for name in &inline_policies {
                if name != READONLY_INLINE_POLICY_NAME {
                    aws_sdk::delete_role_policy(client, role_name, name).await?;
                }
            }
        } else {
            for arn in &desired_arns {
                if !attached.contains(arn) {
                    aws_sdk::attach_role_policy(client, role_name, arn).await?;
                }
            }
            for arn in &attached {
                if !desired_arns.contains(arn) {
                    aws_sdk::detach_role_policy(client, role_name, arn).await?;
                }
            }
            for name in &inline_policies {
                aws_sdk::delete_role_policy(client, role_name, name).await?;
            }
        }
        Ok(())
    }

    /// Assume the read-only role, retrying briefly for IAM eventual consistency right after
    /// the role was created.
    async fn assume_with_retry(
        &self,
        base: &AwsRoleCredentials,
        region: &str,
        role_arn: &str,
        policy_arns: &[String],
        inline_policy: Option<&str>,
        just_created: bool,
    ) -> Result<AwsRoleCredentials> {
        let max_attempts = if just_created { 6 } else { 2 };
        let mut attempt = 1;
        loop {
            match aws_sdk::assume_role_scoped(
                region,
                base,
                role_arn,
                policy_arns,
                inline_policy,
                "roleman-readonly",
            )
            .await
            {
                Ok(creds) => return Ok(creds),
                Err(err) => {
                    if attempt >= max_attempts {
                        return Err(Error::PermissionDrop(format!(
                            "STS AssumeRole on {role_arn} failed: {err}"
                        )));
                    }
                    debug!(attempt, "assume-role not ready yet, retrying after backoff");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    attempt += 1;
                }
            }
        }
    }
}

/// Whether a tag set marks a role as roleman-managed.
fn is_roleman_managed(tags: &[(String, String)]) -> bool {
    tags.iter()
        .any(|(key, value)| key == MANAGED_BY_TAG_KEY && value == MANAGED_BY_TAG_VALUE)
}

/// Value of a tag, if present.
fn tag_value<'a>(tags: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// The caller's session name (the human identity) from an `assumed-role` or `user` caller ARN.
///
/// For SSO, this is the user's email, e.g.
/// `arn:aws:sts::123:assumed-role/AWSReservedSSO_Admin_abc/jane@corp.com` → `jane@corp.com`.
fn caller_session_name(caller_arn: &str) -> Option<String> {
    let resource = caller_arn.splitn(6, ':').nth(5)?;
    let mut parts = resource.splitn(3, '/');
    match parts.next()? {
        "assumed-role" => {
            let _role = parts.next()?;
            parts.next().map(ToString::to_string)
        }
        "user" => parts.next().map(ToString::to_string),
        _ => None,
    }
}

/// Deterministic, per-caller read-only role name: `roleman-ro-<sanitized-owner>-<hash8>`.
///
/// Derived purely from the caller identity (no local state), bounded to IAM's 64-char limit.
/// The hash keeps it unique even when the readable part is truncated or collides after
/// sanitization.
fn readonly_role_name(owner: &str) -> String {
    let mut sanitized: String = owner
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    while sanitized.contains("--") {
        sanitized = sanitized.replace("--", "-");
    }
    let trimmed = sanitized.trim_matches('-');
    let readable: String = trimmed.chars().take(36).collect();
    let readable = readable.trim_matches('-');
    let readable = if readable.is_empty() {
        "user"
    } else {
        readable
    };

    let mut hasher = Sha1::new();
    hasher.update(owner.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    format!("roleman-ro-{readable}-{}", &hash[..8])
}

/// Trust policy for the read-only role: any SSO permission-set role in the account may assume
/// it (read-only, so broad trust is safe). Matches `readonly-probe.sh`.
fn readonly_trust_policy(partition: &str, account_id: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:{partition}:iam::{account_id}:root"}},"Action":"sts:AssumeRole","Condition":{{"ArnLike":{{"aws:PrincipalArn":"arn:{partition}:iam::{account_id}:role/aws-reserved/sso.amazonaws.com/*"}}}}}}]}}"#
    )
}

/// Reject managed resources of an unexpected kind.
fn ensure_iam_role(resource: &ManagedResource) -> Result<()> {
    if resource.kind != "iam-role" {
        return Err(Error::Config(format!(
            "unknown managed resource kind: {}",
            resource.kind
        )));
    }
    Ok(())
}

/// Collect roleman-owned roles (verified by tag) under the reserved path in one account.
async fn collect_managed_roles(
    client: &aws_sdk_iam::Client,
    account_id: &str,
) -> Result<Vec<ManagedResource>> {
    let roles = aws_sdk::list_roles_by_path(client, READONLY_ROLE_PATH).await?;
    let mut resources = Vec::new();
    for role in roles {
        // ListRoles omits tags, so fetch the role to verify roleman ownership.
        let tags = match aws_sdk::get_role(client, &role.name).await? {
            Some(full) => full.tags,
            None => continue,
        };
        // Require the current tag scheme (ManagedBy + owner). Legacy roleman roles that predate
        // the owner tag are ignored on purpose — delete those manually.
        if !is_roleman_managed(&tags) {
            continue;
        }
        let Some(owner) = tag_value(&tags, OWNER_TAG_KEY) else {
            continue;
        };
        let attached = aws_sdk::list_attached_role_policies(client, &role.name)
            .await
            .unwrap_or_default();
        let policies = if attached.is_empty() {
            "no attached policies".to_string()
        } else {
            format!("policies: {}", attached.join(", "))
        };
        let detail = format!("owner: {owner}; {policies}");
        resources.push(ManagedResource {
            kind: "iam-role".to_string(),
            id: role.name,
            arn: role.arn,
            detail,
            account_id: account_id.to_string(),
        });
    }
    Ok(resources)
}

/// Fully delete a role: detach managed policies, delete inline policies, delete the role.
async fn delete_role_completely(client: &aws_sdk_iam::Client, name: &str) -> Result<()> {
    for arn in aws_sdk::list_attached_role_policies(client, name).await? {
        aws_sdk::detach_role_policy(client, name, &arn).await?;
    }
    for policy in aws_sdk::list_role_inline_policies(client, name).await? {
        aws_sdk::delete_role_policy(client, name, &policy).await?;
    }
    aws_sdk::delete_role(client, name).await
}

/// Profile name for a target at a given scope, e.g. `Acme/Admin` or `Acme/Admin@readonly`.
fn profile_name(choice: &RoleChoice, scope: AccessScope, omit_role_name: bool) -> String {
    format!(
        "{}{}",
        aws_config::profile_name_for(choice, omit_role_name),
        scope.profile_suffix()
    )
}

fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(value) => matches!(
            value.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn format_expiration(expiration_ms: u64) -> String {
    let seconds = (expiration_ms / 1000) as i64;
    match time::OffsetDateTime::from_unix_timestamp(seconds) {
        Ok(value) => value
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| expiration_ms.to_string()),
        Err(_) => expiration_ms.to_string(),
    }
}

/// ARN partition (`aws`, `aws-us-gov`, `aws-cn`) from any ARN; defaults to `aws`.
fn partition_of(arn: &str) -> String {
    arn.split(':').nth(1).unwrap_or("aws").to_string()
}

/// Resolve the session policy for `--readonly`: configured policy, or the managed
/// `ReadOnlyAccess` ARN by default. Returns `(policy_arns, inline_policy)`.
fn resolve_readonly_policy(
    identity: &SsoIdentity,
    partition: &str,
) -> (Vec<String>, Option<String>) {
    match &identity.readonly_policy {
        Some(ReadonlyPolicy::PolicyArns(arns)) => (arns.clone(), None),
        Some(ReadonlyPolicy::Inline(doc)) => (Vec::new(), Some(doc.clone())),
        None => (
            vec![format!("arn:{partition}:iam::aws:policy/ReadOnlyAccess")],
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> AwsProvider {
        AwsProvider::new(SsoIdentity {
            name: "work".into(),
            start_url: "https://acme.awsapps.com/start".into(),
            sso_region: "us-east-1".into(),
            provider: crate::config::ProviderKind::Aws,
            readonly_policy: None,
            accounts: Vec::new(),
            ignore_roles: Vec::new(),
        })
    }

    #[test]
    fn env_vars_match_aws_format() {
        let creds = AwsCredentials {
            access_key_id: "AKIA123".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration_ms: 1_700_000_000_000,
            region: "us-east-1".into(),
        };
        let binding = ProfileBinding {
            profile_name: "Acme-Cloud/ReadOnly".into(),
            config_file: Some("/tmp/roleman-aws-config".into()),
        };
        let vars = creds.env_vars(&binding);
        let names: Vec<&str> = vars.iter().map(|var| var.name.as_str()).collect();
        // Order and full set of exported variables.
        assert_eq!(
            names,
            vec![
                "AWS_ACCESS_KEY_ID",
                "AWS_SECRET_ACCESS_KEY",
                "AWS_SESSION_TOKEN",
                "AWS_CREDENTIAL_EXPIRATION",
                "AWS_DEFAULT_REGION",
                "AWS_REGION",
                "AWS_PROFILE",
                "AWS_CONFIG_FILE",
            ]
        );
        assert!(vars.contains(&EnvVar::new("AWS_ACCESS_KEY_ID", "AKIA123")));
        assert!(vars.contains(&EnvVar::new("AWS_PROFILE", "Acme-Cloud/ReadOnly")));
        assert!(vars.contains(&EnvVar::new("AWS_REGION", "us-east-1")));
        assert!(vars.contains(&EnvVar::new("AWS_CONFIG_FILE", "/tmp/roleman-aws-config")));
    }

    #[test]
    fn readonly_scope_suffixes_profile_name() {
        let choice = RoleChoice {
            account_id: "1234".into(),
            account_name: "Acme Cloud".into(),
            role_name: "Admin".into(),
        };
        assert_eq!(
            profile_name(&choice, AccessScope::Full, false),
            "Acme-Cloud/Admin"
        );
        assert_eq!(
            profile_name(&choice, AccessScope::ReadOnly, false),
            "Acme-Cloud/Admin@readonly"
        );
    }

    #[test]
    fn readonly_trust_policy_allows_account_sso_roles() {
        let doc = readonly_trust_policy("aws", "123456789012");
        // Valid JSON, scoped to the account's SSO reserved path.
        let parsed: serde_json::Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(
            parsed["Statement"][0]["Principal"]["AWS"],
            "arn:aws:iam::123456789012:root"
        );
        assert_eq!(
            parsed["Statement"][0]["Condition"]["ArnLike"]["aws:PrincipalArn"],
            "arn:aws:iam::123456789012:role/aws-reserved/sso.amazonaws.com/*"
        );
        assert_eq!(parsed["Statement"][0]["Action"], "sts:AssumeRole");
    }

    #[test]
    fn readonly_role_name_is_per_caller_and_deterministic() {
        let arn_a =
            "arn:aws:sts::123456789012:assumed-role/AWSReservedSSO_Eng_abc123/jane@corp.com";
        let arn_b =
            "arn:aws:sts::123456789012:assumed-role/AWSReservedSSO_Eng_abc123/john@corp.com";

        let owner_a = caller_session_name(arn_a).unwrap();
        let owner_b = caller_session_name(arn_b).unwrap();
        assert_eq!(owner_a, "jane@corp.com");

        let name_a = readonly_role_name(&owner_a);
        let name_b = readonly_role_name(&owner_b);
        // Deterministic for the same caller, distinct across callers, IAM-name-safe and bounded.
        assert_eq!(name_a, readonly_role_name(&owner_a));
        assert_ne!(name_a, name_b);
        assert!(name_a.starts_with("roleman-ro-"));
        assert!(name_a.len() <= 64);
        assert!(
            name_a
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        );
    }

    #[test]
    fn readonly_role_name_bounds_long_callers() {
        let owner = "a".repeat(200);
        let name = readonly_role_name(&owner);
        assert!(name.len() <= 64);
    }

    #[test]
    fn ownership_check_requires_managed_by_tag() {
        assert!(is_roleman_managed(&[(
            "ManagedBy".to_string(),
            "roleman".to_string()
        )]));
        assert!(!is_roleman_managed(&[(
            "ManagedBy".to_string(),
            "someone-else".to_string()
        )]));
        assert!(!is_roleman_managed(&[]));
    }

    #[test]
    fn partition_parsed_from_arn() {
        assert_eq!(partition_of("arn:aws:iam::123:role/x"), "aws");
        assert_eq!(
            partition_of("arn:aws-us-gov:sts::123:assumed-role/x/y"),
            "aws-us-gov"
        );
        assert_eq!(partition_of("not-an-arn"), "aws");
    }

    #[test]
    fn default_readonly_policy_is_managed_readonly_access() {
        let (arns, inline) = resolve_readonly_policy(&provider().identity, "aws");
        assert_eq!(
            arns,
            vec!["arn:aws:iam::aws:policy/ReadOnlyAccess".to_string()]
        );
        assert!(inline.is_none());
    }

    #[test]
    fn console_url_encodes_role() {
        let url = provider().console_url(&RoleChoice {
            account_id: "123456789012".into(),
            account_name: "Acme".into(),
            role_name: "Read Only".into(),
        });
        assert_eq!(
            url,
            "https://acme.awsapps.com/start/#/console?account_id=123456789012&role_name=Read%20Only"
        );
    }
}
