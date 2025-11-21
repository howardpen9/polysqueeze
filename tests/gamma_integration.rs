use reqwest::Client;
use serde_json::Value;

fn run_live_tests_enabled() -> bool {
    std::env::var("RUN_GAMMA_TESTS").is_ok()
}

async fn fetch_gamma(path: &str) -> reqwest::Result<Value> {
    let client = Client::new();
    let response = client
        .get(&format!("https://gamma-api.polymarket.com{}", path))
        .send()
        .await?;

    let response = response.error_for_status()?;
    response.json().await
}

#[tokio::test]
async fn gamma_markets_endpoint() {
    if !run_live_tests_enabled() {
        eprintln!("Skipping Gamma live tests (set RUN_GAMMA_TESTS=1 to enable)");
        return;
    }

    let data = fetch_gamma("/markets?limit=1&offset=0")
        .await
        .expect("Gamma markets call failed");
    assert!(data.is_array(), "Gamma markets must return an array");
    let markets = data.as_array().unwrap();
    assert!(!markets.is_empty(), "Expected at least one market");
    assert!(
        markets[0].get("conditionId").is_some(),
        "Market must expose conditionId"
    );
}

#[tokio::test]
async fn gamma_events_endpoint() {
    if !run_live_tests_enabled() {
        eprintln!("Skipping Gamma live tests (set RUN_GAMMA_TESTS=1 to enable)");
        return;
    }

    let data = fetch_gamma("/events?limit=1&offset=0")
        .await
        .expect("Gamma events call failed");
    assert!(data.is_array());
    assert!(data.as_array().unwrap().first().is_some());
}

#[tokio::test]
async fn gamma_tags_endpoint() {
    if !run_live_tests_enabled() {
        eprintln!("Skipping Gamma live tests (set RUN_GAMMA_TESTS=1 to enable)");
        return;
    }

    let data = fetch_gamma("/tags").await.expect("Gamma tags call failed");
    assert!(data.is_array());
}

#[tokio::test]
async fn gamma_sports_endpoint() {
    if !run_live_tests_enabled() {
        eprintln!("Skipping Gamma live tests (set RUN_GAMMA_TESTS=1 to enable)");
        return;
    }

    let data = fetch_gamma("/sports")
        .await
        .expect("Gamma sports call failed");
    assert!(data.is_array());
}
