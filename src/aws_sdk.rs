use aws_config::Region;
use aws_sdk_sso::types::{AccountInfo, RoleInfo};
use aws_types::SdkConfig;

use crate::error::{Error, Result};
use crate::model::{
    Account, AwsCreateToken, AwsRegisterClient, AwsRoleCredentials, AwsStartDeviceAuthorization,
    Role,
};

pub async fn sdk_config(region: &str) -> Result<SdkConfig> {
    let region = Region::new(region.to_string());
    Ok(aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(region)
        .load()
        .await)
}

pub async fn register_client(region: &str) -> Result<AwsRegisterClient> {
    let config = sdk_config(region).await?;
    let client = aws_sdk_ssooidc::Client::new(&config);
    let output = client
        .register_client()
        .client_name("roleman")
        .client_type("public")
        .send()
        .await
        .map_err(|err| Error::AwsSdk(err.to_string()))?;

    Ok(AwsRegisterClient {
        client_id: output
            .client_id()
            .ok_or_else(|| Error::AwsSdk("missing client_id".into()))?
            .to_string(),
        client_secret: output
            .client_secret()
            .ok_or_else(|| Error::AwsSdk("missing client_secret".into()))?
            .to_string(),
    })
}

pub async fn start_device_authorization(
    region: &str,
    client_id: &str,
    client_secret: &str,
    start_url: &str,
) -> Result<AwsStartDeviceAuthorization> {
    let config = sdk_config(region).await?;
    let client = aws_sdk_ssooidc::Client::new(&config);
    let output = client
        .start_device_authorization()
        .client_id(client_id)
        .client_secret(client_secret)
        .start_url(start_url)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(err.to_string()))?;

    Ok(AwsStartDeviceAuthorization {
        device_code: output
            .device_code()
            .ok_or_else(|| Error::AwsSdk("missing device_code".into()))?
            .to_string(),
        user_code: output
            .user_code()
            .ok_or_else(|| Error::AwsSdk("missing user_code".into()))?
            .to_string(),
        verification_uri_complete: output
            .verification_uri_complete()
            .ok_or_else(|| Error::AwsSdk("missing verification_uri_complete".into()))?
            .to_string(),
        expires_in: output.expires_in() as u64,
        interval: output.interval() as u64,
    })
}

pub async fn create_token(
    region: &str,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> Result<AwsCreateToken> {
    let config = sdk_config(region).await?;
    let client = aws_sdk_ssooidc::Client::new(&config);
    let output = client
        .create_token()
        .client_id(client_id)
        .client_secret(client_secret)
        .device_code(device_code)
        .grant_type("urn:ietf:params:oauth:grant-type:device_code")
        .send()
        .await
        .map_err(|err| Error::AwsSdk(err.to_string()))?;

    Ok(AwsCreateToken {
        access_token: output
            .access_token()
            .ok_or_else(|| Error::AwsSdk("missing access_token".into()))?
            .to_string(),
        expires_in: output.expires_in() as u64,
    })
}

pub async fn list_accounts(access_token: &str, region: &str) -> Result<Vec<Account>> {
    let config = sdk_config(region).await?;
    let client = aws_sdk_sso::Client::new(&config);
    let mut accounts = Vec::new();
    let mut next_token = None;

    loop {
        let mut request = client.list_accounts().access_token(access_token);
        if let Some(token) = next_token.as_deref() {
            request = request.next_token(token);
        }
        let output = request
            .send()
            .await
            .map_err(|err| Error::AwsSdk(err.to_string()))?;

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
    let config = sdk_config(region).await?;
    let client = aws_sdk_sso::Client::new(&config);
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
        let output = request
            .send()
            .await
            .map_err(|err| Error::AwsSdk(err.to_string()))?;

        roles.extend(output.role_list().iter().filter_map(role_from_sdk));

        match output.next_token() {
            Some(token) if !token.is_empty() => next_token = Some(token.to_string()),
            _ => break,
        }
    }

    Ok(roles)
}

pub async fn get_role_credentials(
    access_token: &str,
    region: &str,
    account_id: &str,
    role_name: &str,
) -> Result<AwsRoleCredentials> {
    let config = sdk_config(region).await?;
    let client = aws_sdk_sso::Client::new(&config);
    let output = client
        .get_role_credentials()
        .access_token(access_token)
        .account_id(account_id)
        .role_name(role_name)
        .send()
        .await
        .map_err(|err| Error::AwsSdk(err.to_string()))?;

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
