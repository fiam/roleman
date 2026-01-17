use std::process::Command;

use tracing::{debug, trace};

use crate::error::{Error, Result};
use crate::model::{
    Account, AwsCreateToken, AwsListAccountRoles, AwsListAccounts, AwsRegisterClient,
    AwsRoleCredentials, AwsRoleCredentialsResponse, AwsStartDeviceAuthorization, Role,
};

pub fn register_client(region: &str) -> Result<AwsRegisterClient> {
    let output = run_aws(&[
        "sso-oidc",
        "register-client",
        "--client-name",
        "roleman",
        "--client-type",
        "public",
        "--region",
        region,
    ])?;

    if !output.status.success() {
        return Err(Error::AwsCliOutput(String::from_utf8_lossy(&output.stderr).into()));
    }

    let data: AwsRegisterClient =
        serde_json::from_slice(&output.stdout).map_err(|err| Error::AwsCli(err.to_string()))?;
    Ok(data)
}

pub fn start_device_authorization(
    region: &str,
    client_id: &str,
    client_secret: &str,
    start_url: &str,
) -> Result<AwsStartDeviceAuthorization> {
    let output = run_aws(&[
        "sso-oidc",
        "start-device-authorization",
        "--client-id",
        client_id,
        "--client-secret",
        client_secret,
        "--start-url",
        start_url,
        "--region",
        region,
    ])?;

    if !output.status.success() {
        return Err(Error::AwsCliOutput(String::from_utf8_lossy(&output.stderr).into()));
    }

    let data: AwsStartDeviceAuthorization =
        serde_json::from_slice(&output.stdout).map_err(|err| Error::AwsCli(err.to_string()))?;
    Ok(data)
}

pub fn create_token(
    region: &str,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> Result<AwsCreateToken> {
    let output = run_aws(&[
        "sso-oidc",
        "create-token",
        "--client-id",
        client_id,
        "--client-secret",
        client_secret,
        "--device-code",
        device_code,
        "--grant-type",
        "urn:ietf:params:oauth:grant-type:device_code",
        "--region",
        region,
    ])?;

    if !output.status.success() {
        return Err(Error::AwsCliOutput(String::from_utf8_lossy(&output.stderr).into()));
    }

    let data: AwsCreateToken =
        serde_json::from_slice(&output.stdout).map_err(|err| Error::AwsCli(err.to_string()))?;
    Ok(data)
}

pub fn list_accounts(access_token: &str, region: &str) -> Result<Vec<Account>> {
    let output = run_aws(&[
        "sso",
        "list-accounts",
        "--access-token",
        access_token,
        "--region",
        region,
    ])?;

    if !output.status.success() {
        return Err(Error::AwsCliOutput(String::from_utf8_lossy(&output.stderr).into()));
    }

    let data: AwsListAccounts =
        serde_json::from_slice(&output.stdout).map_err(|err| Error::AwsCli(err.to_string()))?;
    Ok(data
        .account_list
        .into_iter()
        .map(|acct| Account {
            id: acct.account_id,
            name: acct.account_name,
        })
        .collect())
}

pub fn list_account_roles(access_token: &str, region: &str, account_id: &str) -> Result<Vec<Role>> {
    let output = run_aws(&[
        "sso",
        "list-account-roles",
        "--access-token",
        access_token,
        "--region",
        region,
        "--account-id",
        account_id,
    ])?;

    if !output.status.success() {
        return Err(Error::AwsCliOutput(String::from_utf8_lossy(&output.stderr).into()));
    }

    let data: AwsListAccountRoles =
        serde_json::from_slice(&output.stdout).map_err(|err| Error::AwsCli(err.to_string()))?;
    Ok(data
        .role_list
        .into_iter()
        .map(|role| Role { name: role.role_name })
        .collect())
}

pub fn get_role_credentials(
    access_token: &str,
    region: &str,
    account_id: &str,
    role_name: &str,
) -> Result<AwsRoleCredentials> {
    let output = run_aws(&[
        "sso",
        "get-role-credentials",
        "--access-token",
        access_token,
        "--region",
        region,
        "--account-id",
        account_id,
        "--role-name",
        role_name,
    ])?;

    if !output.status.success() {
        return Err(Error::AwsCliOutput(String::from_utf8_lossy(&output.stderr).into()));
    }

    let data: AwsRoleCredentialsResponse =
        serde_json::from_slice(&output.stdout).map_err(|err| Error::AwsCli(err.to_string()))?;
    Ok(data.role_credentials)
}

fn run_aws(args: &[&str]) -> Result<std::process::Output> {
    debug!(args = ?args, "running aws cli");
    let mut command = Command::new("aws");
    command.args(args);
    for (key, _) in std::env::vars() {
        if key.starts_with("AWS_") {
            command.env_remove(key);
        }
    }
    let output = command
        .output()
        .map_err(|err| Error::AwsCli(err.to_string()))?;
    trace!(
        status = %output.status,
        stdout = %String::from_utf8_lossy(&output.stdout),
        stderr = %String::from_utf8_lossy(&output.stderr),
        "aws cli output"
    );
    if !output.status.success() {
        return Err(Error::AwsCliOutput(String::from_utf8_lossy(&output.stderr).into()));
    }
    Ok(output)
}
