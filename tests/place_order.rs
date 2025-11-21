use polysqueeze::client::{ClobClient, OrderArgs};
use polysqueeze::errors::Result;
use polysqueeze::types::{OrderType, PostOrder, Side};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromStr;
use serde_json;
use std::env;

fn env_var(key: &str) -> String {
    env::var(key).expect(&format!("{} must be set for place_order test", key))
}

fn should_run() -> bool {
    env::var("RUN_PLACE_ORDER_TEST").is_ok()
}

#[tokio::test]
async fn place_order_smoke() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping place_order test (set RUN_PLACE_ORDER_TEST=1)");
        return Ok(());
    }

    let base_url =
        env::var("POLY_API_URL").unwrap_or_else(|_| "https://clob.polymarket.com".into());
    let private_key = env_var("POLY_PRIVATE_KEY");
    let api_key = env_var("POLY_API_KEY");
    let chain_id = env::var("POLY_CHAIN_ID")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(137);

    let l1_client = ClobClient::with_l1_headers(&base_url, &private_key, chain_id);
    let creds = l1_client.create_or_derive_api_key(None).await?;
    let mut client = ClobClient::with_l2_headers(&base_url, &private_key, chain_id, creds.clone());
    let funder = env::var("POLY_FUNDER").expect("POLY_FUNDER is not set");
    client.set_funder(&funder)?;

    let book = client
        .get_order_book(
            "55750499609404392022182767653636608406071048880507415981953185669489165869118", // existing market id
        )
        .await?;

    let book_price = (book.bids.first().expect("no bids").price
        + book.asks.first().expect("no asks").price)
        / Decimal::from(2);
    let price_env = env::var("POLY_ORDER_PRICE")
        .ok()
        .and_then(|value| Decimal::from_str(&value).ok());
    let size_env = env::var("POLY_ORDER_SIZE")
        .ok()
        .and_then(|value| Decimal::from_str(&value).ok());
    println!("book price]: {:}", book_price);
    let best_price = price_env.unwrap_or(book_price);
    let min_order_size: Decimal = 2.into();
    let order_size = size_env.unwrap_or(min_order_size);

    let args = OrderArgs::new(
        "55750499609404392022182767653636608406071048880507415981953185669489165869118",
        best_price,
        order_size,
        Side::BUY,
    );
    let signed_order = client.create_order(&args, None, None, None).await?;
    let post_body = PostOrder::new(signed_order.clone(), api_key.clone(), OrderType::GTC);
    println!("order payload: {}", serde_json::to_string(&post_body)?);

    let response = client.post_order(signed_order, OrderType::GTC).await?;
    println!("post_order response: {}", response);

    Ok(())
}
