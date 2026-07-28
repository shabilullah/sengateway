use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

use crate::model::normalize_mac;

#[derive(Debug, Deserialize)]
pub struct PortalQuery {
    pub id: String,
    pub ap: Option<String>,
    pub ssid: Option<String>,
    pub url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortalContext {
    pub mac: String,
    pub ap: Option<String>,
    pub ssid: Option<String>,
    pub redirect_url: Option<String>,
    pub expires_at: i64,
    pub used: bool,
}
impl TryFrom<PortalQuery> for PortalContext {
    type Error = &'static str;
    fn try_from(q: PortalQuery) -> Result<Self, Self::Error> {
        let mac = normalize_mac(&q.id).ok_or("invalid client MAC")?;
        let redirect_url = q
            .url
            .and_then(|raw| Url::parse(&raw).ok())
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .map(Into::into);
        Ok(Self {
            mac,
            ap: q.ap,
            ssid: q.ssid,
            redirect_url,
            expires_at: (OffsetDateTime::now_utc() + time::Duration::minutes(10)).unix_timestamp(),
            used: false,
        })
    }
}
impl PortalContext {
    pub fn consume(&mut self) -> Result<(), &'static str> {
        if self.used || self.expires_at <= OffsetDateTime::now_utc().unix_timestamp() {
            return Err("portal context expired");
        };
        self.used = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_non_http_redirect() {
        let c = super::PortalContext::try_from(super::PortalQuery {
            id: "02:00:00:00:00:01".into(),
            ap: None,
            ssid: None,
            url: Some("javascript:alert(1)".into()),
        })
        .unwrap();
        assert!(c.redirect_url.is_none());
    }
}
