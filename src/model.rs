use serde::Deserialize;

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

#[derive(Debug, Clone)]
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
        format!("{} ({}) — {}", self.account_name, self.account_id, self.role_name)
    }
}

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

#[derive(Debug, Deserialize)]
pub struct AwsRegisterClient {
    #[serde(rename = "clientId")]
    pub client_id: String,
    #[serde(rename = "clientSecret")]
    pub client_secret: String,
    #[serde(rename = "clientSecretExpiresAt")]
    pub client_secret_expires_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct AwsStartDeviceAuthorization {
    #[serde(rename = "deviceCode")]
    pub device_code: String,
    #[serde(rename = "userCode")]
    pub user_code: String,
    #[serde(rename = "verificationUri")]
    pub verification_uri_complete: String,
    #[serde(rename = "expiresIn")]
    pub expires_in: u64,
    #[serde(rename = "interval")]
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
pub struct AwsCreateToken {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "expiresIn")]
    pub expires_in: u64,
}

#[derive(Debug, Clone)]
pub struct EnvVars {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration_ms: u64,
    pub region: String,
    pub profile_name: String,
    pub config_file: Option<String>,
}

impl EnvVars {
    pub fn from_role_credentials(
        creds: &AwsRoleCredentials,
        profile_name: &str,
        region: &str,
    ) -> Self {
        Self {
            access_key_id: creds.access_key_id.clone(),
            secret_access_key: creds.secret_access_key.clone(),
            session_token: creds.session_token.clone(),
            expiration_ms: creds.expiration,
            region: region.to_string(),
            profile_name: profile_name.to_string(),
            config_file: None,
        }
    }

    pub fn to_export_lines(&self) -> String {
        let expiration = format_expiration(self.expiration_ms);
        let mut lines = vec![
            format!("export AWS_ACCESS_KEY_ID={}", self.access_key_id),
            format!("export AWS_SECRET_ACCESS_KEY={}", self.secret_access_key),
            format!("export AWS_SESSION_TOKEN={}", self.session_token),
            format!("export AWS_CREDENTIAL_EXPIRATION={}", expiration),
            format!("export AWS_DEFAULT_REGION={}", self.region),
            format!("export AWS_REGION={}", self.region),
            format!("export AWS_PROFILE={}", self.profile_name),
        ];
        if let Some(path) = &self.config_file {
            lines.push(format!("export AWS_CONFIG_FILE={}", path));
        }
        lines.join("\n")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_export_format() {
        let env = EnvVars {
            access_key_id: "AKIA123".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration_ms: 1_700_000_000_000,
            region: "us-east-1".into(),
            profile_name: "Acme-Cloud/ReadOnly".into(),
            config_file: Some("/tmp/roleman-aws-config".into()),
        };
        let output = env.to_export_lines();
        assert!(output.contains("AWS_ACCESS_KEY_ID=AKIA123"));
        assert!(output.contains("AWS_SECRET_ACCESS_KEY=secret"));
        assert!(output.contains("AWS_SESSION_TOKEN=token"));
        assert!(output.contains("AWS_CREDENTIAL_EXPIRATION="));
        assert!(output.contains("AWS_DEFAULT_REGION=us-east-1"));
        assert!(output.contains("AWS_REGION=us-east-1"));
        assert!(output.contains("AWS_PROFILE=Acme-Cloud/ReadOnly"));
        assert!(output.contains("AWS_CONFIG_FILE=/tmp/roleman-aws-config"));
    }
}
