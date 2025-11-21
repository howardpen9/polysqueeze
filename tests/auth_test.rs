use polysqueeze::client::ClobClient;
use polysqueeze::errors::Result;
use std::env;

fn env_var(key: &str) -> String {
    env::var(key).expect(&format!("{} must be set for auth test", key))
}

fn should_run() -> bool {
    env::var("RUN_AUTH_TEST").is_ok()
}

#[tokio::test]
async fn derived_auth_matches_env() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping auth test (set RUN_AUTH_TEST=1)");
        return Ok(());
    }

    let base_url =
        env::var("POLY_API_URL").unwrap_or_else(|_| "https://clob.polymarket.com".into());
    let private_key = env_var("POLY_PRIVATE_KEY");
    let expected_api_key = env_var("POLY_API_KEY");
    let expected_secret = env_var("POLY_API_SECRET");
    let expected_passphrase = env_var("POLY_API_PASSPHRASE");
    let chain_id = env::var("POLY_CHAIN_ID")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(137);

    let client = ClobClient::with_l1_headers(&base_url, &private_key, chain_id);
    let creds = client.create_or_derive_api_key(None).await?;

    assert_eq!(
        creds.api_key, expected_api_key,
        "Derived API key should match the provided env var"
    );
    assert_eq!(
        creds.secret, expected_secret,
        "Derived secret should match the provided env var"
    );
    assert_eq!(
        creds.passphrase, expected_passphrase,
        "Derived passphrase should match the provided env var"
    );

    Ok(())
}

#[tokio::test]
async fn post_api_key_with_l1_headers_succeeds() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping auth test (set RUN_AUTH_TEST=1)");
        return Ok(());
    }

    let base_url =
        env::var("POLY_API_URL").unwrap_or_else(|_| "https://clob.polymarket.com".into());
    let private_key = env_var("POLY_PRIVATE_KEY");
    let chain_id = env::var("POLY_CHAIN_ID")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(137);

    let client = ClobClient::with_l1_headers(&base_url, &private_key, chain_id);
    let creds = client.create_or_derive_api_key(None).await?;

    assert!(!creds.api_key.is_empty(), "API key should be returned");
    assert!(!creds.secret.is_empty(), "Secret should be returned");
    assert!(
        !creds.passphrase.is_empty(),
        "Passphrase should be returned"
    );
    println!("creds are: {:?}", creds);
    Ok(())
}

#[tokio::test]
async fn l2_get_api_keys_succeeds() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping auth test (set RUN_AUTH_TEST=1)");
        return Ok(());
    }

    let base_url =
        env::var("POLY_API_URL").unwrap_or_else(|_| "https://clob.polymarket.com".into());
    let private_key = env_var("POLY_PRIVATE_KEY");
    let chain_id = env::var("POLY_CHAIN_ID")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(137);

    let l1_client = ClobClient::with_l1_headers(&base_url, &private_key, chain_id);
    let creds = l1_client.create_or_derive_api_key(None).await?;

    let l2_client = ClobClient::with_l2_headers(&base_url, &private_key, chain_id, creds.clone());
    let keys = l2_client.get_api_keys().await?;

    assert!(
        keys.contains(&creds.api_key),
        "Derived API key should appear in api-keys list"
    );

    println!("keys: {:?}", keys);

    Ok(())
}
