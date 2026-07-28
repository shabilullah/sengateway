use rand::Rng;
use sha2::{Digest, Sha256};

pub const INVALID_COUPON: &str = "Coupon invalid or unavailable";
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn normalize_mac(input: &str) -> Option<String> {
    let raw: String = input.chars().filter(|c| *c != ':' && *c != '-').collect();
    if raw.len() != 12 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes: Vec<u8> = (0..12)
        .step_by(2)
        .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).ok())
        .collect::<Option<_>>()?;
    if bytes[0] & 1 != 0 || bytes.iter().all(|b| *b == 0) {
        return None;
    }
    Some(
        bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

pub fn generate_coupon() -> (String, [u8; 32], String) {
    let mut rng = rand::rng();
    let compact: String = (0..12)
        .map(|_| CROCKFORD[rng.random_range(0..32)] as char)
        .collect();
    let rendered = format!("{}-{}-{}", &compact[..4], &compact[4..8], &compact[8..]);
    let hash = coupon_hash(&compact);
    (rendered, hash, compact[8..].to_owned())
}

pub fn coupon_hash(input: &str) -> [u8; 32] {
    let normalized: String = input
        .chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    Sha256::digest(normalized.as_bytes()).into()
}

pub fn validity_minutes(value: u32, unit: &str) -> Option<i64> {
    let factor = match unit {
        "HOURS" => 60,
        "DAYS" => 1440,
        "WEEKS" => 10080,
        _ => return None,
    };
    let minutes = i64::from(value).checked_mul(factor)?;
    (60..=524_160).contains(&minutes).then_some(minutes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn mac_validation() {
        assert_eq!(
            super::normalize_mac("02-aa-bb-cc-dd-ee").unwrap(),
            "02:AA:BB:CC:DD:EE"
        );
        assert!(super::normalize_mac("ff:ff:ff:ff:ff:ff").is_none());
    }
    #[test]
    fn coupon_normalization() {
        assert_eq!(
            super::coupon_hash("abcd-efgh-jkmn"),
            super::coupon_hash("ABCDEFGHJKMN")
        );
    }
    #[test]
    fn validity_bounds() {
        assert_eq!(super::validity_minutes(52, "WEEKS"), Some(524160));
        assert_eq!(super::validity_minutes(53, "WEEKS"), None);
    }
}
