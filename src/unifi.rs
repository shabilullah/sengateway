use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::{env, fs, sync::Arc, time::Duration};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct UnifiClient {
    http: Client,
    base: String,
    site: String,
    key: SecretString,
    last_success: Arc<RwLock<Option<OffsetDateTime>>>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ClientRecord {
    pub id: String,
    #[serde(rename = "macAddress")]
    pub mac_address: String,
    #[serde(default)]
    pub access: Access,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Access {
    #[serde(default)]
    pub authorized: bool,
}
#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    data: Vec<ClientRecord>,
}
#[derive(Deserialize)]
struct SitesPage {
    #[serde(default)]
    data: Vec<SiteRecord>,
}

#[derive(Deserialize)]
struct SiteRecord {
    id: String,
}
#[derive(Serialize)]
struct Action<'a> {
    action: &'a str,
    #[serde(rename = "timeLimitMinutes", skip_serializing_if = "Option::is_none")]
    minutes: Option<i64>,
}
#[derive(Debug, Error)]
pub enum UnifiError {
    #[error("UniFi request failed")]
    Request,
    #[error("UniFi returned HTTP {0}")]
    Http(StatusCode),
    #[error("client is not known to UniFi")]
    NotFound,
}

impl UnifiClient {
    pub fn new(base: String, site: String, key: SecretString) -> Result<Self, UnifiError> {
        let mut builder = Client::builder().timeout(Duration::from_secs(10));
        if let Some(path) = env::var_os("UNIFI_CA_CERT_PATH") {
            let pem = fs::read(path).map_err(|_| UnifiError::Request)?;
            let certificate =
                reqwest::Certificate::from_pem(&pem).map_err(|_| UnifiError::Request)?;
            builder = builder.add_root_certificate(certificate);
        }
        let http = builder.build().map_err(|_| UnifiError::Request)?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').into(),
            site,
            key,
            last_success: Arc::new(RwLock::new(None)),
        })
    }
    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("X-API-Key", self.key.expose_secret())
    }
    pub async fn site_check(&self) -> Result<(), UnifiError> {
        let r = self
            .request(reqwest::Method::GET, format!("{}/sites", self.base))
            .send()
            .await
            .map_err(|_| UnifiError::Request)?;
        let sites: SitesPage = self
            .accept(r)
            .await?
            .json()
            .await
            .map_err(|_| UnifiError::Request)?;
        sites
            .data
            .iter()
            .any(|site| site.id == self.site)
            .then_some(())
            .ok_or(UnifiError::NotFound)
    }
    pub async fn readiness_check(&self) -> Result<(), UnifiError> {
        let r = self
            .request(
                reqwest::Method::GET,
                format!("{}/sites/{}/clients", self.base, self.site),
            )
            .query(&[("pageSize", "1")])
            .send()
            .await
            .map_err(|_| UnifiError::Request)?;
        self.accept(r).await.map(|_| ())
    }
    pub async fn resolve_mac(&self, mac: &str) -> Result<ClientRecord, UnifiError> {
        let filter = format!("macAddress.eq('{mac}')");
        let r = self
            .request(
                reqwest::Method::GET,
                format!("{}/sites/{}/clients", self.base, self.site),
            )
            .query(&[("filter", filter)])
            .send()
            .await
            .map_err(|_| UnifiError::Request)?;
        let r = self.accept(r).await?;
        let page: Page = r.json().await.map_err(|_| UnifiError::Request)?;
        page.data
            .into_iter()
            .find(|c| c.mac_address.eq_ignore_ascii_case(mac))
            .ok_or(UnifiError::NotFound)
    }
    pub async fn authorize(&self, id: &str, minutes: i64) -> Result<(), UnifiError> {
        if minutes < 1 {
            return Err(UnifiError::Request);
        }
        self.action(
            id,
            Action {
                action: "AUTHORIZE_GUEST_ACCESS",
                minutes: Some(minutes),
            },
        )
        .await
    }
    pub async fn unauthorize(&self, id: &str) -> Result<(), UnifiError> {
        let result = self
            .action(
                id,
                Action {
                    action: "UNAUTHORIZE_GUEST_ACCESS",
                    minutes: None,
                },
            )
            .await;
        match result {
            Err(UnifiError::Http(StatusCode::NOT_FOUND | StatusCode::CONFLICT)) => {
                let client = self.get_client(id).await?;
                if client.access.authorized {
                    result
                } else {
                    Ok(())
                }
            }
            other => other,
        }
    }
    async fn get_client(&self, id: &str) -> Result<ClientRecord, UnifiError> {
        let r = self
            .request(
                reqwest::Method::GET,
                format!("{}/sites/{}/clients/{}", self.base, self.site, id),
            )
            .send()
            .await
            .map_err(|_| UnifiError::Request)?;
        self.accept(r)
            .await?
            .json()
            .await
            .map_err(|_| UnifiError::Request)
    }
    async fn action(&self, id: &str, body: Action<'_>) -> Result<(), UnifiError> {
        let r = self
            .request(
                reqwest::Method::POST,
                format!("{}/sites/{}/clients/{}/actions", self.base, self.site, id),
            )
            .json(&body)
            .send()
            .await
            .map_err(|_| UnifiError::Request)?;
        self.accept(r).await.map(|_| ())
    }
    async fn accept(&self, r: reqwest::Response) -> Result<reqwest::Response, UnifiError> {
        if !r.status().is_success() {
            return Err(UnifiError::Http(r.status()));
        }
        *self.last_success.write().await = Some(OffsetDateTime::now_utc());
        Ok(r)
    }
    pub async fn last_success(&self) -> Option<OffsetDateTime> {
        *self.last_success.read().await
    }
    pub fn redacted_endpoint(&self) -> String {
        reqwest::Url::parse(&self.base)
            .map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("invalid")))
            .unwrap_or_else(|_| "invalid".into())
    }
    pub fn site(&self) -> &str {
        &self.site
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    #[tokio::test]
    async fn site_check_finds_site_in_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sites"))
            .and(header("X-API-Key", "key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "wanted-site"}, {"id": "other-site"}]
            })))
            .mount(&server)
            .await;

        let client = UnifiClient::new(
            server.uri(),
            "wanted-site".into(),
            SecretString::from("key"),
        )
        .unwrap();
        client.site_check().await.unwrap();
    }
}
