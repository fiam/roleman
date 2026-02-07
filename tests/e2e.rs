mod support;

use roleman::aws_sdk;
use roleman::{MockServerOptions, start_mock_server};

#[tokio::test]
async fn e2e_sso_flow_uses_mock_endpoints() {
    let _lock = support::lock_env();
    let server = start_mock_server(MockServerOptions {
        host: "127.0.0.1".to_string(),
        port: 0,
    })
    .await
    .expect("failed to start mock server");
    let base = format!("http://{}", server.addr());
    let previous_sso = std::env::var("ROLEMAN_SSO_ENDPOINT").ok();
    let previous_imds = std::env::var("AWS_EC2_METADATA_DISABLED").ok();
    unsafe {
        std::env::set_var("ROLEMAN_SSO_ENDPOINT", format!("{}/sso", base));
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
    }

    let region = "us-east-1";
    let token = "mock-access-token";

    let accounts = aws_sdk::list_accounts(token, region)
        .await
        .expect("list_accounts failed");
    assert!(!accounts.is_empty());

    let account = accounts
        .iter()
        .find(|entry| entry.id == "111111111111")
        .expect("expected mock account");
    let roles = aws_sdk::list_account_roles(token, region, &account.id)
        .await
        .expect("list_account_roles failed");
    assert!(roles.iter().any(|role| role.name == "Admin"));

    let creds = aws_sdk::get_role_credentials(token, region, &account.id, "Admin")
        .await
        .expect("get_role_credentials failed");
    assert_eq!(creds.access_key_id, "ASIAMOCKACCESSKEY");

    unsafe {
        if let Some(value) = previous_sso {
            std::env::set_var("ROLEMAN_SSO_ENDPOINT", value);
        } else {
            std::env::remove_var("ROLEMAN_SSO_ENDPOINT");
        }
        if let Some(value) = previous_imds {
            std::env::set_var("AWS_EC2_METADATA_DISABLED", value);
        } else {
            std::env::remove_var("AWS_EC2_METADATA_DISABLED");
        }
    }
    server.shutdown().await.expect("mock server shutdown");
}
