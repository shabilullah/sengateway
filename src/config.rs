use std::{env, fs, net::IpAddr, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::RngCore;
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

fn setting(name: &str, errors: &mut Vec<String>) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => {
            errors.push(format!("{name} is missing"));
            None
        }
    }
}

fn persistent_secret(
    name: &str,
    file_name: &str,
    bytes: usize,
    secret_dir: Option<&Path>,
    errors: &mut Vec<String>,
) -> Option<String> {
    if let Ok(value) = env::var(name)
        && !value.trim().is_empty()
    {
        return Some(value);
    }
    let Some(secret_dir) = secret_dir else {
        errors.push(format!("{name} is missing"));
        return None;
    };
    let path = secret_dir.join(file_name);
    match fs::read_to_string(&path) {
        Ok(value) if !value.trim().is_empty() => return Some(value.trim_end().into()),
        Ok(_) => {
            errors.push(format!("{} is empty", path.display()));
            return None;
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            errors.push(format!("cannot read {}: {error}", path.display()));
            return None;
        }
        Err(_) => {}
    }

    let mut random = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut random);
    let value = STANDARD.encode(random);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path).and_then(|mut file| {
        use std::io::Write;
        file.write_all(value.as_bytes())?;
        file.sync_all()
    }) {
        Ok(()) => Some(value),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::read_to_string(&path)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(|value| value.trim_end().into())
                .or_else(|| {
                    errors.push(format!("cannot read generated {}", path.display()));
                    None
                })
        }
        Err(error) => {
            errors.push(format!("cannot create {}: {error}", path.display()));
            None
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let mut errors = Vec::new();
        let get = setting;
        let secret_dir = env::var_os("SENGATEWAY_SECRET_DIR").map(std::path::PathBuf::from);
        let public_base_url =
            get("PUBLIC_BASE_URL", &mut errors).and_then(|v| match Url::parse(&v) {
                Ok(url) if url.scheme() == "https" && url.path() == "/" => Some(url),
                _ => {
                    errors.push("PUBLIC_BASE_URL must be an https origin".into());
                    None
                }
            });
        let database_url = get("DATABASE_URL", &mut errors);
        let session_secret = persistent_secret(
            "SESSION_SECRET",
            ".session-secret",
            48,
            secret_dir.as_deref(),
            &mut errors,
        )
        .and_then(|v| {
            if v.len() >= 32 {
                Some(v.into_bytes())
            } else {
                errors.push("SESSION_SECRET must contain at least 32 bytes".into());
                None
            }
        });
        let encryption_key = persistent_secret(
            "SETUP_ENCRYPTION_KEY",
            ".setup-encryption-key",
            32,
            secret_dir.as_deref(),
            &mut errors,
        )
        .and_then(|v| match STANDARD.decode(v) {
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

    #[test]
    fn persistent_secret_is_created_once() {
        let dir = std::env::temp_dir().join(format!("sengateway-secret-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let mut errors = Vec::new();
        let first =
            super::persistent_secret("TEST_SECRET", "secret", 32, Some(&dir), &mut errors).unwrap();
        let second =
            super::persistent_secret("TEST_SECRET", "secret", 32, Some(&dir), &mut errors).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join("secret"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, first)
                .unwrap()
                .len(),
            32
        );
        assert!(errors.is_empty());
    }
}
