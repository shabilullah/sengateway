use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::{env, fs, process::Stdio, sync::Arc, time::Duration};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::process::Command;
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
    data: Vec<Site>,
    #[serde(rename = "totalCount")]
    total_count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Site {
    pub id: String,
    pub name: String,
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
    #[error("UniFi request timed out")]
    Timeout,
    #[error("UniFi connection failed")]
    Connect,
    #[error("UniFi returned HTTP {0}")]
    Http(StatusCode),
    #[error("client is not known to UniFi")]
    NotFound,
}

impl UnifiClient {
    pub fn new(
        base: String,
        site: String,
        key: SecretString,
        pinned_certificate_pem: Option<&[u8]>,
    ) -> Result<Self, UnifiError> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(10))
            .tls_built_in_webpki_certs(false)
            .tls_built_in_native_certs(true);
        let configured = env::var_os("UNIFI_CA_CERT_PATH")
            .map(fs::read)
            .transpose()
            .map_err(|_| UnifiError::Request)?;
        if let Some(pem) = pinned_certificate_pem.or(configured.as_deref()) {
            let certificate =
                reqwest::Certificate::from_pem(pem).map_err(|_| UnifiError::Request)?;
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
    pub async fn capture_certificate(base: &str) -> Result<Vec<u8>, UnifiError> {
        let url = reqwest::Url::parse(base).map_err(|_| UnifiError::Request)?;
        if url.scheme() != "https" {
            return Err(UnifiError::Request);
        }
        let host = url.host_str().ok_or(UnifiError::Request)?;
        let port = url.port_or_known_default().ok_or(UnifiError::Request)?;
        let mut command = Command::new("openssl");
        command
            .args([
                "s_client",
                "-connect",
                &format!("{host}:{port}"),
                "-servername",
                host,
                "-showcerts",
            ])
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(12), command.output())
            .await
            .map_err(|_| UnifiError::Request)?
            .map_err(|_| UnifiError::Request)?;
        let stdout = String::from_utf8(output.stdout).map_err(|_| UnifiError::Request)?;
        let begin = stdout
            .find("-----BEGIN CERTIFICATE-----")
            .ok_or(UnifiError::Request)?;
        let relative_end = stdout[begin..]
            .find("-----END CERTIFICATE-----")
            .ok_or(UnifiError::Request)?;
        let end = begin + relative_end + "-----END CERTIFICATE-----".len();
        Ok(format!("{}\n", &stdout[begin..end]).into_bytes())
    }
    fn transport(error: reqwest::Error) -> UnifiError {
        if error.is_timeout() {
            UnifiError::Timeout
        } else if error.is_connect() {
            UnifiError::Connect
        } else {
            UnifiError::Request
        }
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("X-API-Key", self.key.expose_secret())
    }
    pub async fn sites(&self) -> Result<Vec<Site>, UnifiError> {
        const LIMIT: usize = 200;
        let mut sites = Vec::new();
        loop {
            let r = self
                .request(reqwest::Method::GET, format!("{}/sites", self.base))
                .query(&[("offset", sites.len()), ("limit", LIMIT)])
                .send()
                .await
                .map_err(Self::transport)?;
            let page: SitesPage = self
                .accept(r)
                .await?
                .json()
                .await
                .map_err(|_| UnifiError::Request)?;
            let page_len = page.data.len();
            sites.extend(page.data);
            if page_len == 0 || page.total_count.is_none_or(|total| sites.len() >= total) {
                return Ok(sites);
            }
        }
    }

    pub async fn site_check(&self) -> Result<(), UnifiError> {
        self.sites()
            .await?
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
            .map_err(Self::transport)?;
        self.accept(r).await.map(|_| ())
    }
    pub async fn resolve_mac(&self, mac: &str) -> Result<ClientRecord, UnifiError> {
        let filter = format!("macAddress.eq('{}')", mac.to_ascii_lowercase());
        let mut delay = Duration::from_millis(500);
        for attempt in 0..4 {
            let result = async {
                let r = self
                    .request(
                        reqwest::Method::GET,
                        format!("{}/sites/{}/clients", self.base, self.site),
                    )
                    .query(&[("filter", &filter)])
                    .send()
                    .await
                    .map_err(Self::transport)?;
                let r = self.accept(r).await?;
                let page: Page = r.json().await.map_err(|_| UnifiError::Request)?;
                page.data
                    .into_iter()
                    .find(|c| c.mac_address.eq_ignore_ascii_case(mac))
                    .ok_or(UnifiError::NotFound)
            }
            .await;
            match result {
                Ok(client) => return Ok(client),
                Err(error)
                    if attempt < 3
                        && (matches!(
                            error,
                            UnifiError::Request
                                | UnifiError::NotFound
                                | UnifiError::Http(StatusCode::TOO_MANY_REQUESTS)
                        ) || matches!(&error, UnifiError::Http(status) if status.is_server_error())) =>
                {
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    }
    pub async fn authorize(&self, id: &str, minutes: Option<i64>) -> Result<(), UnifiError> {
        if minutes.is_some_and(|value| value < 1) {
            return Err(UnifiError::Request);
        }
        self.action(
            id,
            Action {
                action: "AUTHORIZE_GUEST_ACCESS",
                minutes,
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
            .map_err(Self::transport)?;
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
            .map_err(Self::transport)?;
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
    async fn resolve_mac_uses_lowercase_filter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sites/site/clients"))
            .and(wiremock::matchers::query_param(
                "filter",
                "macAddress.eq('76:6a:09:73:5c:88')",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "client",
                    "macAddress": "76:6a:09:73:5c:88",
                    "access": {"authorized": false}
                }]
            })))
            .mount(&server)
            .await;
        let client =
            UnifiClient::new(server.uri(), "site".into(), SecretString::from("key"), None).unwrap();

        assert_eq!(
            client.resolve_mac("76:6A:09:73:5C:88").await.unwrap().id,
            "client"
        );
    }

    #[tokio::test]
    async fn resolve_mac_retries_transient_controller_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/sites/site/clients"))
            .respond_with(move |_: &wiremock::Request| {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(503)
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "data": [{
                            "id": "client",
                            "macAddress": "76:6a:09:73:5c:88"
                        }]
                    }))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        let client =
            UnifiClient::new(server.uri(), "site".into(), SecretString::from("key"), None).unwrap();

        assert_eq!(
            client.resolve_mac("76:6A:09:73:5C:88").await.unwrap().id,
            "client"
        );
    }

    #[tokio::test]
    async fn site_check_finds_site_in_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sites"))
            .and(header("X-API-Key", "key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "wanted-site", "name": "Main Office"},
                    {"id": "other-site", "name": "Warehouse"}
                ]
            })))
            .mount(&server)
            .await;

        let client = UnifiClient::new(
            server.uri(),
            "wanted-site".into(),
            SecretString::from("key"),
            None,
        )
        .unwrap();
        client.site_check().await.unwrap();
    }

    #[tokio::test]
    async fn sites_returns_ids_and_network_names() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sites"))
            .and(wiremock::matchers::query_param("offset", "0"))
            .and(wiremock::matchers::query_param("limit", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "main-id", "name": "Main Office"},
                    {"id": "branch-id", "name": "Branch Network"}
                ]
            })))
            .mount(&server)
            .await;
        let client =
            UnifiClient::new(server.uri(), String::new(), SecretString::from("key"), None).unwrap();

        assert_eq!(
            client.sites().await.unwrap(),
            vec![
                Site {
                    id: "main-id".into(),
                    name: "Main Office".into()
                },
                Site {
                    id: "branch-id".into(),
                    name: "Branch Network".into()
                },
            ]
        );
    }

    #[tokio::test]
    async fn sites_fetches_every_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sites"))
            .and(wiremock::matchers::query_param("offset", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalCount": 2,
                "data": [{"id": "main-id", "name": "Main Office"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/sites"))
            .and(wiremock::matchers::query_param("offset", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalCount": 2,
                "data": [{"id": "branch-id", "name": "Branch Network"}]
            })))
            .mount(&server)
            .await;
        let client =
            UnifiClient::new(server.uri(), String::new(), SecretString::from("key"), None).unwrap();

        assert_eq!(client.sites().await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn authorize_without_duration_omits_time_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sites/site/clients/client/actions"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "action": "AUTHORIZE_GUEST_ACCESS"
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client =
            UnifiClient::new(server.uri(), "site".into(), SecretString::from("key"), None).unwrap();

        client.authorize("client", None).await.unwrap();
    }
}
