use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub access_token: String,
    pub expires_at: String,
    pub region: String,
}

#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Role {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoleChoice {
    pub account_id: String,
    pub account_name: String,
    pub role_name: String,
}

impl RoleChoice {
    pub fn new(account: &Account, role: &Role) -> Self {
        Self {
            account_id: account.id.clone(),
            account_name: account.name.clone(),
            role_name: role.name.clone(),
        }
    }

    pub fn label(&self) -> String {
        format!(
            "{} ({}) — {}",
            self.account_name, self.account_id, self.role_name
        )
    }
}

/// Raw role credentials as returned by the AWS SSO `GetRoleCredentials` API.
#[derive(Debug, Deserialize)]
pub struct AwsRoleCredentials {
    #[serde(rename = "accessKeyId")]
    pub access_key_id: String,
    #[serde(rename = "secretAccessKey")]
    pub secret_access_key: String,
    #[serde(rename = "sessionToken")]
    pub session_token: String,
    #[serde(rename = "expiration")]
    pub expiration: u64,
}
