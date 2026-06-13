use aws_config::Region;
use aws_sdk_sso::types::{AccountInfo, RoleInfo};
use aws_smithy_runtime_api::client::result::SdkError as SmithySdkError;
use aws_smithy_runtime_api::http::Response as SmithyResponse;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use aws_types::SdkConfig;
use aws_types::request_id::RequestId;

use crate::error::{Error, Result};
use crate::model::{Account, AwsRoleCredentials, Role};

pub async fn sdk_config(region: &str) -> Result<SdkConfig> {
    let region = Region::new(region.to_string());
    Ok(aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(region)
        .load()
        .await)
}

async fn sso_client(region: &str) -> Result<aws_sdk_sso::Client> {
    let config = sdk_config(region).await?;
    let mut builder = aws_sdk_sso::config::Builder::from(&config);
    if let Ok(url) = std::env::var("ROLEMAN_SSO_ENDPOINT")
        && !url.is_empty()
    {
        builder = builder.endpoint_url(url);
    }
    Ok(aws_sdk_sso::Client::from_conf(builder.build()))
}

pub async fn list_accounts(access_token: &str, region: &str) -> Result<Vec<Account>> {
    let client = sso_client(region).await?;
    let mut accounts = Vec::new();
    let mut next_token = None;

    loop {
        let mut request = client.list_accounts().access_token(access_token);
        if let Some(token) = next_token.as_deref() {
            request = request.next_token(token);
        }
        let output: aws_sdk_sso::operation::list_accounts::ListAccountsOutput =
            retry_sdk(|| request.clone().send(), 5).await?;

        accounts.extend(output.account_list().iter().filter_map(account_from_sdk));

        match output.next_token() {
            Some(token) if !token.is_empty() => next_token = Some(token.to_string()),
            _ => break,
        }
    }

    Ok(accounts)
}

pub async fn list_account_roles(
    access_token: &str,
    region: &str,
    account_id: &str,
) -> Result<Vec<Role>> {
    let client = sso_client(region).await?;
    let mut roles = Vec::new();
    let mut next_token = None;

    loop {
        let mut request = client
            .list_account_roles()
            .access_token(access_token)
            .account_id(account_id);
        if let Some(token) = next_token.as_deref() {
            request = request.next_token(token);
        }
        let output: aws_sdk_sso::operation::list_account_roles::ListAccountRolesOutput =
            retry_sdk(|| request.clone().send(), 5).await?;

        roles.extend(output.role_list().iter().filter_map(role_from_sdk));

        match output.next_token() {
            Some(token) if !token.is_empty() => next_token = Some(token.to_string()),
            _ => break,
        }
    }

    Ok(roles)
}

async fn retry_sdk<F, Fut, T, E>(mut call: F, max_attempts: usize) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, SmithySdkError<E, SmithyResponse>>>,
    E: ProvideErrorMetadata + std::fmt::Debug + std::fmt::Display,
{
    let mut attempt = 1;
    loop {
        match call().await {
            Ok(output) => return Ok(output),
            Err(err) => {
                let message = format_sdk_error(&err);
                if attempt >= max_attempts || !is_throttle_error(err.meta().code(), &message) {
                    return Err(Error::AwsSdk(message));
                }
                let backoff_ms = 500_u64.saturating_mul(2_u64.pow((attempt - 1) as u32));
                tracing::debug!(attempt, backoff_ms, "throttled by aws sdk, backing off");
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                attempt += 1;
            }
        }
    }
}

fn is_throttle_error(code: Option<&str>, message: &str) -> bool {
    if let Some(code) = code
        && matches!(
            code,
            "TooManyRequestsException" | "ThrottlingException" | "Throttling"
        )
    {
        return true;
    }
    message.contains("TooManyRequests")
        || message.contains("TooManyRequestsException")
        || message.contains("Throttling")
        || message.contains("Rate exceeded")
}

pub async fn get_role_credentials(
    access_token: &str,
    region: &str,
    account_id: &str,
    role_name: &str,
) -> Result<AwsRoleCredentials> {
    let client = sso_client(region).await?;
    let output = client
        .get_role_credentials()
        .access_token(access_token)
        .account_id(account_id)
        .role_name(role_name)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;

    let creds = output
        .role_credentials()
        .ok_or_else(|| Error::AwsSdk("missing role_credentials".into()))?;

    Ok(AwsRoleCredentials {
        access_key_id: creds
            .access_key_id()
            .ok_or_else(|| Error::AwsSdk("missing access_key_id".into()))?
            .to_string(),
        secret_access_key: creds
            .secret_access_key()
            .ok_or_else(|| Error::AwsSdk("missing secret_access_key".into()))?
            .to_string(),
        session_token: creds
            .session_token()
            .ok_or_else(|| Error::AwsSdk("missing session_token".into()))?
            .to_string(),
        expiration: creds.expiration() as u64,
    })
}

/// Build an STS client authenticated with explicit (base SSO role) credentials,
/// rather than the ambient credential chain.
async fn sts_client(region: &str, creds: &AwsRoleCredentials) -> Result<aws_sdk_sts::Client> {
    let region_obj = Region::new(region.to_string());
    let static_creds = aws_sdk_sts::config::Credentials::new(
        creds.access_key_id.clone(),
        creds.secret_access_key.clone(),
        Some(creds.session_token.clone()),
        None,
        "roleman-sso",
    );
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(region_obj)
        .credentials_provider(static_creds)
        .load()
        .await;
    let mut builder = aws_sdk_sts::config::Builder::from(&config);
    if let Ok(url) = std::env::var("ROLEMAN_STS_ENDPOINT")
        && !url.is_empty()
    {
        builder = builder.endpoint_url(url);
    }
    Ok(aws_sdk_sts::Client::from_conf(builder.build()))
}

/// Return the ARN of the identity the given credentials represent.
///
/// For SSO credentials this is an `assumed-role` ARN like
/// `arn:aws:sts::123456789012:assumed-role/AWSReservedSSO_Admin_abc123/user@example.com`.
pub async fn get_caller_arn(region: &str, creds: &AwsRoleCredentials) -> Result<String> {
    let client = sts_client(region, creds).await?;
    let output = client
        .get_caller_identity()
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    output
        .arn()
        .map(|arn| arn.to_string())
        .ok_or_else(|| Error::AwsSdk("missing caller arn".into()))
}

/// Re-assume `role_arn` with a restrictive session policy, producing scoped-down credentials.
///
/// A session policy can only *restrict* the effective permissions of the role, never expand
/// them, so this is how `--readonly` drops write access. Requires the role's trust policy to
/// permit re-assumption; callers should surface a clear error if STS denies the request.
pub async fn assume_role_scoped(
    region: &str,
    creds: &AwsRoleCredentials,
    role_arn: &str,
    policy_arns: &[String],
    inline_policy: Option<&str>,
    session_name: &str,
) -> Result<AwsRoleCredentials> {
    let client = sts_client(region, creds).await?;
    let mut request = client
        .assume_role()
        .role_arn(role_arn)
        .role_session_name(session_name);
    for arn in policy_arns {
        request = request.policy_arns(
            aws_sdk_sts::types::PolicyDescriptorType::builder()
                .arn(arn)
                .build(),
        );
    }
    if let Some(policy) = inline_policy {
        request = request.policy(policy);
    }
    let output = request
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    let creds = output
        .credentials()
        .ok_or_else(|| Error::AwsSdk("missing assume-role credentials".into()))?;
    Ok(AwsRoleCredentials {
        access_key_id: creds.access_key_id().to_string(),
        secret_access_key: creds.secret_access_key().to_string(),
        session_token: creds.session_token().to_string(),
        expiration: (creds.expiration().secs() as u64).saturating_mul(1000),
    })
}

/// Summary of an IAM role roleman cares about: identity plus tags (for ownership checks).
#[derive(Debug, Clone)]
pub struct RoleSummary {
    pub name: String,
    pub arn: String,
    pub path: String,
    pub tags: Vec<(String, String)>,
}

/// Build an IAM client. With `creds`, authenticates as the given (base SSO role) credentials;
/// without, uses the ambient credential chain (whatever `rl` exported into the shell).
///
/// IAM is a global service, but the SDK still needs a region for request signing.
async fn build_iam_client(
    region: &str,
    creds: Option<&AwsRoleCredentials>,
) -> Result<aws_sdk_iam::Client> {
    let region_obj = Region::new(region.to_string());
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest()).region(region_obj);
    if let Some(creds) = creds {
        loader = loader.credentials_provider(aws_sdk_iam::config::Credentials::new(
            creds.access_key_id.clone(),
            creds.secret_access_key.clone(),
            Some(creds.session_token.clone()),
            None,
            "roleman-sso",
        ));
    }
    let config = loader.load().await;
    let mut builder = aws_sdk_iam::config::Builder::from(&config);
    if let Ok(url) = std::env::var("ROLEMAN_IAM_ENDPOINT")
        && !url.is_empty()
    {
        builder = builder.endpoint_url(url);
    }
    Ok(aws_sdk_iam::Client::from_conf(builder.build()))
}

/// IAM client authenticated with explicit base-role credentials (used while downscoping).
pub async fn iam_client_static(
    region: &str,
    creds: &AwsRoleCredentials,
) -> Result<aws_sdk_iam::Client> {
    build_iam_client(region, Some(creds)).await
}

/// Account id of the ambient credentials, used to identify which account `roleman cleanup`
/// (single-account) should operate on.
pub async fn ambient_account(region: &str) -> Result<String> {
    let region_obj = Region::new(region.to_string());
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(region_obj)
        .load()
        .await;
    let mut builder = aws_sdk_sts::config::Builder::from(&config);
    if let Ok(url) = std::env::var("ROLEMAN_STS_ENDPOINT")
        && !url.is_empty()
    {
        builder = builder.endpoint_url(url);
    }
    let client = aws_sdk_sts::Client::from_conf(builder.build());
    let output = client
        .get_caller_identity()
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    output
        .account()
        .map(ToString::to_string)
        .ok_or_else(|| Error::AwsSdk("missing caller account".into()))
}

fn role_summary_from_sdk(role: &aws_sdk_iam::types::Role) -> RoleSummary {
    RoleSummary {
        name: role.role_name().to_string(),
        arn: role.arn().to_string(),
        path: role.path().to_string(),
        tags: role
            .tags()
            .iter()
            .map(|tag| (tag.key().to_string(), tag.value().to_string()))
            .collect(),
    }
}

/// Fetch a role by name, or `None` if it does not exist.
pub async fn get_role(client: &aws_sdk_iam::Client, name: &str) -> Result<Option<RoleSummary>> {
    match client.get_role().role_name(name).send().await {
        Ok(output) => Ok(output.role().map(role_summary_from_sdk)),
        Err(err) if err.code() == Some("NoSuchEntity") => Ok(None),
        Err(err) => Err(Error::AwsSdk(format_sdk_error(&err))),
    }
}

/// Create a role at `path` with the given trust policy and tags.
pub async fn create_role(
    client: &aws_sdk_iam::Client,
    name: &str,
    path: &str,
    assume_role_policy: &str,
    description: &str,
    tags: &[(&str, &str)],
) -> Result<()> {
    let mut request = client
        .create_role()
        .role_name(name)
        .path(path)
        .assume_role_policy_document(assume_role_policy)
        .description(description);
    for (key, value) in tags {
        request = request.tags(
            aws_sdk_iam::types::Tag::builder()
                .key(*key)
                .value(*value)
                .build()
                .map_err(|err| Error::AwsSdk(err.to_string()))?,
        );
    }
    request
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    Ok(())
}

pub async fn attach_role_policy(
    client: &aws_sdk_iam::Client,
    name: &str,
    policy_arn: &str,
) -> Result<()> {
    client
        .attach_role_policy()
        .role_name(name)
        .policy_arn(policy_arn)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    Ok(())
}

pub async fn detach_role_policy(
    client: &aws_sdk_iam::Client,
    name: &str,
    policy_arn: &str,
) -> Result<()> {
    client
        .detach_role_policy()
        .role_name(name)
        .policy_arn(policy_arn)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    Ok(())
}

pub async fn put_role_policy(
    client: &aws_sdk_iam::Client,
    name: &str,
    policy_name: &str,
    policy_document: &str,
) -> Result<()> {
    client
        .put_role_policy()
        .role_name(name)
        .policy_name(policy_name)
        .policy_document(policy_document)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    Ok(())
}

pub async fn delete_role_policy(
    client: &aws_sdk_iam::Client,
    name: &str,
    policy_name: &str,
) -> Result<()> {
    client
        .delete_role_policy()
        .role_name(name)
        .policy_name(policy_name)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    Ok(())
}

/// ARNs of managed policies attached to a role.
pub async fn list_attached_role_policies(
    client: &aws_sdk_iam::Client,
    name: &str,
) -> Result<Vec<String>> {
    let mut arns = Vec::new();
    let mut marker = None;
    loop {
        let mut request = client.list_attached_role_policies().role_name(name);
        if let Some(value) = marker.as_deref() {
            request = request.marker(value);
        }
        let output = request
            .send()
            .await
            .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
        arns.extend(
            output
                .attached_policies()
                .iter()
                .filter_map(|policy| policy.policy_arn().map(ToString::to_string)),
        );
        match output.marker() {
            Some(value) if output.is_truncated() => marker = Some(value.to_string()),
            _ => break,
        }
    }
    Ok(arns)
}

/// Names of inline policies on a role.
pub async fn list_role_inline_policies(
    client: &aws_sdk_iam::Client,
    name: &str,
) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut marker = None;
    loop {
        let mut request = client.list_role_policies().role_name(name);
        if let Some(value) = marker.as_deref() {
            request = request.marker(value);
        }
        let output = request
            .send()
            .await
            .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
        names.extend(output.policy_names().iter().cloned());
        match output.marker() {
            Some(value) if output.is_truncated() => marker = Some(value.to_string()),
            _ => break,
        }
    }
    Ok(names)
}

/// Roles under a given path prefix (e.g. `/roleman/`).
pub async fn list_roles_by_path(
    client: &aws_sdk_iam::Client,
    path_prefix: &str,
) -> Result<Vec<RoleSummary>> {
    let mut roles = Vec::new();
    let mut marker = None;
    loop {
        let mut request = client.list_roles().path_prefix(path_prefix);
        if let Some(value) = marker.as_deref() {
            request = request.marker(value);
        }
        let output = request
            .send()
            .await
            .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
        roles.extend(output.roles().iter().map(role_summary_from_sdk));
        match output.marker() {
            Some(value) if output.is_truncated() => marker = Some(value.to_string()),
            _ => break,
        }
    }
    Ok(roles)
}

pub async fn delete_role(client: &aws_sdk_iam::Client, name: &str) -> Result<()> {
    client
        .delete_role()
        .role_name(name)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(format_sdk_error(&err)))?;
    Ok(())
}

fn account_from_sdk(account: &AccountInfo) -> Option<Account> {
    Some(Account {
        id: account.account_id()?.to_string(),
        name: account.account_name()?.to_string(),
    })
}

fn role_from_sdk(role: &RoleInfo) -> Option<Role> {
    Some(Role {
        name: role.role_name()?.to_string(),
    })
}

fn format_sdk_error<E>(err: &E) -> String
where
    E: ProvideErrorMetadata + std::fmt::Display + std::fmt::Debug,
{
    let mut parts = Vec::new();
    let mut base = err.to_string();
    if base == "service error" {
        base = format!("{err:?}");
    }
    parts.push(base);
    let meta = err.meta();
    if let Some(code) = meta.code() {
        parts.push(format!("code={code}"));
    }
    if let Some(message) = meta.message() {
        parts.push(format!("message={message}"));
    }
    if let Some(request_id) = meta.request_id() {
        parts.push(format!("request_id={request_id}"));
    }
    parts.join(" | ")
}
