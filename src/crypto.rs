use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("secret encryption failed")]
    Encrypt,
    #[error("secret decryption failed")]
    Decrypt,
    #[error("decrypted secret is not UTF-8")]
    Utf8,
}

#[allow(deprecated)]
pub fn encrypt(key: &[u8; 32], secret: &SecretString) -> Result<(Vec<u8>, [u8; 12]), CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("fixed key length");
    let mut nonce = [0_u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), secret.expose_secret().as_bytes())
        .map_err(|_| CryptoError::Encrypt)?;
    Ok((ciphertext, nonce))
}

#[allow(deprecated)]
pub fn decrypt(
    key: &[u8; 32],
    ciphertext: &[u8],
    nonce: &[u8],
) -> Result<SecretString, CryptoError> {
    if nonce.len() != 12 {
        return Err(CryptoError::Decrypt);
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("fixed key length");
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(plaintext)
        .map(SecretString::from)
        .map_err(|_| CryptoError::Utf8)
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;
    #[test]
    fn round_trip_and_random_nonce() {
        let key = [7; 32];
        let secret = secrecy::SecretString::from("private".to_owned());
        let (a, na) = super::encrypt(&key, &secret).unwrap();
        let (b, nb) = super::encrypt(&key, &secret).unwrap();
        assert_ne!(na, nb);
        assert_ne!(a, b);
        assert_eq!(
            super::decrypt(&key, &a, &na).unwrap().expose_secret(),
            "private"
        );
    }
}
