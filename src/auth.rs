//! Authentication and cryptographic utilities for Polymarket API
//!
//! This module provides EIP-712 signing, HMAC authentication, and header generation
//! for secure communication with the Polymarket CLOB API.

use crate::errors::{PolyError, Result};
use crate::types::ApiCredentials;
use alloy_primitives::{Address, U256, hex::encode_prefixed};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{eip712_domain, sol};
use base64::engine::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// Header constants
const POLY_ADDR_HEADER: &str = "poly_address";
const POLY_SIG_HEADER: &str = "poly_signature";
const POLY_TS_HEADER: &str = "poly_timestamp";
const POLY_NONCE_HEADER: &str = "poly_nonce";
const POLY_API_KEY_HEADER: &str = "poly_api_key";
const POLY_PASS_HEADER: &str = "poly_passphrase";

type Headers = HashMap<&'static str, String>;

fn decode_api_secret(secret: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE
        .decode(secret)
        .or_else(|_| URL_SAFE_NO_PAD.decode(secret))
        .or_else(|_| STANDARD.decode(secret))
        .unwrap_or_else(|_| secret.as_bytes().to_vec())
}

fn format_body_for_signature<T>(body: &T) -> Result<String>
where
    T: ?Sized + Serialize,
{
    serde_json::to_string(body)
        .map_err(|e| PolyError::parse(format!("Failed to serialize body: {}", e), None))
}

sol! {
    struct ClobAuth {
        address address;
        string timestamp;
        uint256 nonce;
        string message;
    }
}

sol! {
    struct Order {
        uint256 salt;
        address maker;
        address signer;
        address taker;
        uint256 tokenId;
        uint256 makerAmount;
        uint256 takerAmount;
        uint256 expiration;
        uint256 nonce;
        uint256 feeRateBps;
        uint8 side;
        uint8 signatureType;
    }
}

/// Get current Unix timestamp in seconds
pub fn get_current_unix_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

/// Sign CLOB authentication message using EIP-712
pub fn sign_clob_auth_message(
    signer: &PrivateKeySigner,
    timestamp: String,
    nonce: U256,
) -> Result<String> {
    let message = "This message attests that I control the given wallet".to_string();
    let polygon = 137;

    let auth_struct = ClobAuth {
        address: signer.address(),
        timestamp,
        nonce,
        message,
    };

    let domain = eip712_domain!(
        name: "ClobAuthDomain",
        version: "1",
        chain_id: polygon,
    );

    let signature = signer
        .sign_typed_data_sync(&auth_struct, &domain)
        .map_err(|e| PolyError::crypto(format!("EIP-712 signature failed: {}", e)))?;

    Ok(encode_prefixed(signature.as_bytes()))
}

/// Sign order message using EIP-712
pub fn sign_order_message(
    signer: &PrivateKeySigner,
    order: Order,
    chain_id: u64,
    verifying_contract: Address,
) -> Result<String> {
    let domain = eip712_domain!(
        name: "Polymarket CTF Exchange",
        version: "1",
        chain_id: chain_id,
        verifying_contract: verifying_contract,
    );

    let signature = signer
        .sign_typed_data_sync(&order, &domain)
        .map_err(|e| PolyError::crypto(format!("Order signature failed: {}", e)))?;

    Ok(encode_prefixed(signature.as_bytes()))
}

/// Build HMAC signature for L2 authentication
pub fn build_hmac_signature<T>(
    secret: &str,
    timestamp: u64,
    method: &str,
    request_path: &str,
    body: Option<&T>,
) -> Result<String>
where
    T: ?Sized + Serialize,
{
    let mut mac = Hmac::<Sha256>::new_from_slice(&decode_api_secret(secret))
        .map_err(|e| PolyError::crypto(format!("Invalid HMAC key: {}", e)))?;

    // Build the message to sign: timestamp + method + path + body
    let body_string = match body {
        Some(b) => format_body_for_signature(b)?,
        None => String::new(),
    };
    let message = format!(
        "{}{}{}{}",
        timestamp,
        method.to_uppercase(),
        request_path,
        body_string
    );

    mac.update(message.as_bytes());
    let result = mac.finalize();
    Ok(URL_SAFE.encode(result.into_bytes()))
}

/// Create L1 headers for authentication (using private key signature)
pub fn create_l1_headers(signer: &PrivateKeySigner, nonce: Option<U256>) -> Result<Headers> {
    let timestamp = get_current_unix_time_secs().to_string();
    let nonce = nonce.unwrap_or(U256::ZERO);
    let signature = sign_clob_auth_message(signer, timestamp.clone(), nonce)?;
    let address = encode_prefixed(signer.address().as_slice());

    Ok(HashMap::from([
        (POLY_ADDR_HEADER, address),
        (POLY_SIG_HEADER, signature),
        (POLY_TS_HEADER, timestamp),
        (POLY_NONCE_HEADER, nonce.to_string()),
    ]))
}

/// Create L2 headers for API calls (using API key and HMAC)
pub fn create_l2_headers<T>(
    signer: &PrivateKeySigner,
    api_creds: &ApiCredentials,
    method: &str,
    req_path: &str,
    body: Option<&T>,
) -> Result<Headers>
where
    T: ?Sized + Serialize,
{
    let address = encode_prefixed(signer.address().as_slice());
    let timestamp = get_current_unix_time_secs();

    let hmac_signature =
        build_hmac_signature(&api_creds.secret, timestamp, method, req_path, body)?;

    Ok(HashMap::from([
        (POLY_ADDR_HEADER, address),
        (POLY_SIG_HEADER, hmac_signature),
        (POLY_TS_HEADER, timestamp.to_string()),
        (POLY_API_KEY_HEADER, api_creds.api_key.clone()),
        (POLY_PASS_HEADER, api_creds.passphrase.clone()),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    const PY_ORDER_BODY: &str = "{\"order\":{\"salt\":123456789,\"maker\":\"0xabc\",\"signer\":\"0xabc\",\"taker\":\"0x0000000000000000000000000000000000000000\",\"tokenId\":\"1111\",\"makerAmount\":\"500\",\"takerAmount\":\"5000\",\"expiration\":\"0\",\"nonce\":\"0\",\"feeRateBps\":\"0\",\"side\":\"BUY\",\"signatureType\":1,\"signature\":\"0xdeadbeef\"},\"owner\":\"owner-key\",\"orderType\":\"GTC\"}";
    const PY_ORDER_SIGNATURE: &str = "DI6rkXwOkY27WwKZsKr8Gtn5KPl-ca2yAqHD5ECszR0=";
    const PY_ORDER_MESSAGE_HASH: &str =
        "838d12287413f1af44c2487c7b06c49189d8781280703a81fba93af84fa4faea";

    #[test]
    fn test_unix_timestamp() {
        let timestamp = get_current_unix_time_secs();
        assert!(timestamp > 1_600_000_000); // Should be after 2020
    }

    #[test]
    fn test_hmac_signature() {
        let result =
            build_hmac_signature::<String>("test_secret", 1234567890, "GET", "/test", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_hmac_signature_with_body() {
        let body = r#"{"test": "data"}"#;
        let result = build_hmac_signature("test_secret", 1234567890, "POST", "/orders", Some(body));
        assert!(result.is_ok());
        let signature = result.unwrap();
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_hmac_signature_consistency() {
        let secret = "test_secret";
        let timestamp = 1234567890;
        let method = "GET";
        let path = "/test";

        let sig1 = build_hmac_signature::<String>(secret, timestamp, method, path, None).unwrap();
        let sig2 = build_hmac_signature::<String>(secret, timestamp, method, path, None).unwrap();

        // Same inputs should produce same signature
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_hmac_signature_different_inputs() {
        let secret = "test_secret";
        let timestamp = 1234567890;

        let sig1 = build_hmac_signature::<String>(secret, timestamp, "GET", "/test", None).unwrap();
        let sig2 =
            build_hmac_signature::<String>(secret, timestamp, "POST", "/test", None).unwrap();
        let sig3 =
            build_hmac_signature::<String>(secret, timestamp, "GET", "/other", None).unwrap();

        // Different inputs should produce different signatures
        assert_ne!(sig1, sig2);
        assert_ne!(sig1, sig3);
        assert_ne!(sig2, sig3);
    }

    #[test]
    fn test_decode_api_secret_with_urlsafe_padding() {
        assert_eq!(decode_api_secret("cQ=="), b"q".to_vec());
    }

    #[test]
    fn test_format_body_for_signature_matches_python() {
        let body = json!({ "order": { "foo": 1 } });
        let formatted = format_body_for_signature(&body).expect("Formatting should succeed");
        assert_eq!(formatted, "{\"order\":{\"foo\":1}}");
    }

    #[derive(Serialize)]
    struct SamplePostOrder<'a> {
        order: SampleOrder<'a>,
        owner: &'a str,
        #[serde(rename = "orderType")]
        order_type: &'a str,
    }

    #[allow(non_snake_case)]
    #[derive(Serialize)]
    struct SampleOrder<'a> {
        salt: u64,
        maker: &'a str,
        signer: &'a str,
        taker: &'a str,
        tokenId: &'a str,
        makerAmount: &'a str,
        takerAmount: &'a str,
        expiration: &'a str,
        nonce: &'a str,
        feeRateBps: &'a str,
        side: &'a str,
        signatureType: u8,
        signature: &'a str,
    }

    #[test]
    fn test_order_hmac_matches_python_reference() {
        let body = SamplePostOrder {
            order: SampleOrder {
                salt: 123456789,
                maker: "0xabc",
                signer: "0xabc",
                taker: "0x0000000000000000000000000000000000000000",
                tokenId: "1111",
                makerAmount: "500",
                takerAmount: "5000",
                expiration: "0",
                nonce: "0",
                feeRateBps: "0",
                side: "BUY",
                signatureType: 1,
                signature: "0xdeadbeef",
            },
            owner: "owner-key",
            order_type: "GTC",
        };

        let formatted = format_body_for_signature(&body).expect("Formatting should succeed");
        assert_eq!(formatted, PY_ORDER_BODY);

        let mut hasher = Sha256::new();
        hasher.update(format!("{}POST{}{}", 123456, "/order", PY_ORDER_BODY).as_bytes());
        let hash_hex = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<String>();
        assert_eq!(hash_hex, PY_ORDER_MESSAGE_HASH);

        let signature = build_hmac_signature("c2VjcmV0", 123456, "POST", "/order", Some(&body))
            .expect("HMAC should be computed");
        assert_eq!(signature, PY_ORDER_SIGNATURE);
    }

    #[test]
    fn test_create_l1_headers() {
        use alloy_primitives::U256;
        use alloy_signer_local::PrivateKeySigner;

        let private_key = "0x1234567890123456789012345678901234567890123456789012345678901234";
        let signer: PrivateKeySigner = private_key.parse().expect("Valid private key");

        let result = create_l1_headers(&signer, Some(U256::from(12345)));
        assert!(result.is_ok());

        let headers = result.unwrap();
        assert!(headers.contains_key("poly_address"));
        assert!(headers.contains_key("poly_signature"));
        assert!(headers.contains_key("poly_timestamp"));
        assert!(headers.contains_key("poly_nonce"));
    }

    #[test]
    fn test_create_l1_headers_different_nonces() {
        use alloy_primitives::U256;
        use alloy_signer_local::PrivateKeySigner;

        let private_key = "0x1234567890123456789012345678901234567890123456789012345678901234";
        let signer: PrivateKeySigner = private_key.parse().expect("Valid private key");

        let headers_1 = create_l1_headers(&signer, Some(U256::from(12345))).unwrap();
        let headers_2 = create_l1_headers(&signer, Some(U256::from(54321))).unwrap();

        // Different nonces should produce different signatures
        assert_ne!(
            headers_1.get("poly_signature"),
            headers_2.get("poly_signature")
        );

        // But same address
        assert_eq!(headers_1.get("poly_address"), headers_2.get("poly_address"));
    }

    #[test]
    fn test_create_l2_headers() {
        use alloy_signer_local::PrivateKeySigner;

        let private_key = "0x1234567890123456789012345678901234567890123456789012345678901234";
        let signer: PrivateKeySigner = private_key.parse().expect("Valid private key");

        let api_creds = ApiCredentials {
            api_key: "test_key".to_string(),
            secret: "test_secret".to_string(),
            passphrase: "test_passphrase".to_string(),
        };

        let result = create_l2_headers::<String>(&signer, &api_creds, "/test", "GET", None);
        assert!(result.is_ok());

        let headers = result.unwrap();
        assert!(headers.contains_key("poly_api_key"));
        assert!(headers.contains_key("poly_signature"));
        assert!(headers.contains_key("poly_timestamp"));
        assert!(headers.contains_key("poly_passphrase"));

        assert_eq!(headers.get("poly_api_key").unwrap(), "test_key");
        assert_eq!(headers.get("poly_passphrase").unwrap(), "test_passphrase");
    }

    #[test]
    fn test_eip712_signature_format() {
        use alloy_primitives::U256;
        use alloy_signer_local::PrivateKeySigner;

        let private_key = "0x1234567890123456789012345678901234567890123456789012345678901234";
        let signer: PrivateKeySigner = private_key.parse().expect("Valid private key");

        // Test that we can create and sign EIP-712 messages
        let result = create_l1_headers(&signer, Some(U256::from(12345)));
        assert!(result.is_ok());

        let headers = result.unwrap();
        let signature = headers.get("poly_signature").unwrap();

        // EIP-712 signatures should be hex strings of specific length
        assert!(signature.starts_with("0x"));
        assert_eq!(signature.len(), 132); // 0x + 130 hex chars = 132 total
    }

    #[test]
    fn test_timestamp_generation() {
        let ts1 = get_current_unix_time_secs();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let ts2 = get_current_unix_time_secs();

        // Timestamps should be increasing
        assert!(ts2 >= ts1);

        // Should be reasonable current time (after 2020, before 2030)
        assert!(ts1 > 1_600_000_000);
        assert!(ts1 < 1_900_000_000);
    }
}
