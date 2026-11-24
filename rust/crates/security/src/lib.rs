#![forbid(unsafe_code)]

use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

pub fn valid_nonce(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn fresh_nonce() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("OS randomness unavailable: {e}"))?;
    Ok(hex::encode(bytes))
}

pub fn proof(token: &str, role: &str, client_nonce: &str, server_nonce: &str) -> Result<String> {
    if !valid_nonce(token)
        || !matches!(role, "client" | "server")
        || !valid_nonce(client_nonce)
        || !valid_nonce(server_nonce)
    {
        bail!("Invalid authentication challenge");
    }
    let mut mac = HmacSha256::new_from_slice(&hex::decode(token)?)?;
    mac.update(format!("snapgent-auth-v1:{role}:{client_nonce}:{server_nonce}").as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

pub fn constant_match(expected: &str, provided: &str) -> bool {
    provided.is_ascii() && bool::from(expected.as_bytes().ct_eq(provided.as_bytes()))
}

pub fn trusted_origin(origin: Option<&str>, allowed: &[String]) -> bool {
    match origin {
        None => true,
        Some(origin) => allowed.iter().any(|candidate| candidate == origin),
    }
}

pub fn authorized_http(header: Option<&str>, token: &str) -> bool {
    let Some((scheme, value)) = header.and_then(|s| s.split_once(' ')) else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("bearer") {
        return constant_match(token, value);
    }
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let Ok(decoded) = STANDARD.decode(value) else {
        return false;
    };
    let Ok(decoded) = String::from_utf8(decoded) else {
        return false;
    };
    let Some((user, password)) = decoded.split_once(':') else {
        return false;
    };
    user == "snapgent" && constant_match(token, password)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_authentication_parity() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../app-cases/authentication.json")).unwrap();
        let token = fixture["token"].as_str().unwrap();
        let client = fixture["client_nonce"].as_str().unwrap();
        let server = fixture["server_nonce"].as_str().unwrap();
        for role in ["client", "server"] {
            let expected = fixture[format!("{role}_proof")].as_str().unwrap();
            assert_eq!(proof(token, role, client, server).unwrap(), expected);
        }
    }
    #[test]
    fn nonces_and_role_separation() {
        let token = "ab".repeat(32);
        let a = fresh_nonce().unwrap();
        let b = fresh_nonce().unwrap();
        assert_ne!(a, b);
        assert!(valid_nonce(&a));
        assert_ne!(
            proof(&token, "server", &a, &b).unwrap(),
            proof(&token, "client", &a, &b).unwrap()
        );
        assert!(proof(&token, "invalid", &a, &b).is_err());
    }
    #[test]
    fn exact_origin_policy() {
        let allowed = vec!["chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()];
        assert!(trusted_origin(Some(&allowed[0]), &allowed));
        assert!(!trusted_origin(Some("https://example.com"), &allowed));
        assert!(!trusted_origin(
            Some("chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            &allowed
        ));
    }
    #[test]
    fn http_credentials() {
        let token = "ab".repeat(32);
        assert!(authorized_http(Some(&format!("Bearer {token}")), &token));
        let basic = STANDARD.encode(format!("snapgent:{token}"));
        assert!(authorized_http(Some(&format!("Basic {basic}")), &token));
        assert!(!authorized_http(None, &token));
        assert!(!authorized_http(Some("Bearer invalid"), &token));
    }
}
