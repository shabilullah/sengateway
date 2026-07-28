use std::{env, net::IpAddr};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use thiserror::Error;
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub public_base_url: Url,
    pub database_url: String,
    pub session_secret: Vec<u8>,
    pub encryption_key: [u8; 32],
    pub cookie_secure: bool,
    pub trusted_proxy_ip: IpAddr,
}

#[derive(Debug, Error)]
#[error("invalid application environment: {0}")]
pub struct ConfigError(String);

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let mut errors = Vec::new();
        let get = |name: &str, errors: &mut Vec<String>| match env::var(name) {
            Ok(v) if !v.trim().is_empty() => Some(v),
            _ => {
                errors.push(format!("{name} is missing"));
                None
            }
        };
        let public_base_url =
            get("PUBLIC_BASE_URL", &mut errors).and_then(|v| match Url::parse(&v) {
                Ok(url) if url.scheme() == "https" && url.path() == "/" => Some(url),
                _ => {
                    errors.push("PUBLIC_BASE_URL must be an https origin".into());
                    None
                }
            });
        let database_url = get("DATABASE_URL", &mut errors);
        let session_secret = get("SESSION_SECRET", &mut errors).and_then(|v| {
            if v.len() >= 32 {
                Some(v.into_bytes())
            } else {
                errors.push("SESSION_SECRET must contain at least 32 bytes".into());
                None
            }
        });
        let encryption_key =
            get("SETUP_ENCRYPTION_KEY", &mut errors).and_then(|v| match STANDARD.decode(v) {
                Ok(bytes) if bytes.len() == 32 => Some(bytes.try_into().expect("length checked")),
                _ => {
                    errors.push("SETUP_ENCRYPTION_KEY must be base64 for exactly 32 bytes".into());
                    None
                }
            });
        let cookie_secure = match env::var("COOKIE_SECURE").as_deref() {
            Ok("true") => Some(true),
            Ok("false") => Some(false),
            _ => {
                errors.push("COOKIE_SECURE must be true or false".into());
                None
            }
        };
        let trusted_proxy_ip = get("TRUSTED_PROXY_IP", &mut errors).and_then(|v| match v.parse() {
            Ok(ip) => Some(ip),
            Err(_) => {
                errors.push("TRUSTED_PROXY_IP must be an IP address".into());
                None
            }
        });
        if errors.is_empty() {
            Ok(Self {
                public_base_url: public_base_url.unwrap(),
                database_url: database_url.unwrap(),
                session_secret: session_secret.unwrap(),
                encryption_key: encryption_key.unwrap(),
                cookie_secure: cookie_secure.unwrap(),
                trusted_proxy_ip: trusted_proxy_ip.unwrap(),
            })
        } else {
            Err(ConfigError(errors.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn encryption_key_size_is_exact() {
        assert_eq!(32, 256 / 8);
    }
}
