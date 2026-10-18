use super::{
    cloud::{check, entry, Page},
    crypto, number, text, Download, Session,
};
use crate::model::*;
use serde_json::{json, Value};

impl Session {
    pub async fn pan115_identity(&self) -> Result<(String, String), String> {
        let data = check(
            self.provider,
            self.json(
                self.transport
                    .client
                    .get(self.endpoint("https://webapi.115.com/user/info"))
                    .query(&[("ct", "json")])
                    .header("Referer", "https://115.com/"),
            )
            .await?,
        )?;
        let uid = self
            .cookie_value(&["https://115.com/", "https://webapi.115.com/"], "UID")
            .and_then(|value| value.split('_').next().map(String::from))
            .filter(|id| !id.is_empty())
            .ok_or("请先在官方窗口登录 115 网盘")?;
        let name = text(&data["data"]);
        Ok((
            uid,
            if name.is_empty() {
                "115 账号".into()
            } else {
                name
            },
        ))
    }
    pub async fn pan115_page(&self, path: &str, id: &str, page: u32) -> Result<Page, String> {
        let data = check(
            self.provider,
            self.json(
                self.transport
                    .client
                    .get(self.endpoint("https://webapi.115.com/files"))
                    .header("Referer", "https://115.com/")
                    .query(&[
                        ("aid", "1".into()),
                        ("cid", id.into()),
                        ("offset", ((page - 1) * 100).to_string()),
                        ("limit", "100".into()),
                        ("show_dir", "1".into()),
                        ("o", "user_ptime".into()),
                        ("asc", "0".into()),
                        ("format", "json".into()),
                        ("snap", "0".into()),
                        ("natsort", "1".into()),
                        ("count_folders", "1".into()),
                    ]),
            )
            .await?,
        )?;
        let rows = data["data"].as_array().ok_or("115 文件列表无效")?;
        Ok(Page {
            entries: rows
                .iter()
                .map(|raw| entry(self.provider, raw, path))
                .collect::<Result<_, _>>()?,
            next: (number(&data["count"]) > page as u64 * 100
                || (data.get("count").is_none() && rows.len() == 100))
                .then(|| page.to_string()),
        })
    }
    pub async fn pan115_share(
        &self,
        share: &ShareLink,
        parent: &str,
    ) -> Result<ShareListing, String> {
        super::cloud::identifier(parent)?;
        let mut files = Vec::new();
        let mut title = "115 分享".to_string();
        for page in 0..200 {
            let value = check(
                self.provider,
                self.json(
                    self.transport
                        .client
                        .get(self.endpoint("https://webapi.115.com/share/snap"))
                        .header("Referer", "https://115.com/")
                        .query(&[
                            ("share_code", share.id.clone()),
                            ("receive_code", share.passcode.clone()),
                            ("cid", parent.into()),
                            ("offset", (page * 100).to_string()),
                            ("limit", "100".into()),
                            ("format", "json".into()),
                        ]),
                )
                .await?,
            )?;
            let data = &value["data"];
            if let Some(name) = data["shareinfo"]["share_title"].as_str() {
                title = name.into()
            }
            let rows = data["list"]
                .as_array()
                .ok_or("115 分享列表无效，请检查提取码或分享有效期")?;
            for raw in rows {
                let is_dir = number(&raw["fc"]) == 0;
                let id = text(if is_dir { &raw["cid"] } else { &raw["fid"] });
                super::cloud::identifier(&id)?;
                files.push(ShareFile {
                    id,
                    name: text(&raw["n"]),
                    size: number(&raw["s"]),
                    is_dir,
                    parent_id: parent.into(),
                    token: String::new(),
                });
            }
            if rows.len() < 100
                || (data.get("count").is_some() && number(&data["count"]) <= files.len() as u64)
            {
                return Ok(ShareListing {
                    share: share.clone(),
                    title,
                    parent_id: parent.into(),
                    files,
                    stoken: String::new(),
                });
            }
        }
        Err("115 分享目录超过 20000 项，请分批下载".into())
    }
    async fn pan115_app_download(&self, path: &str, payload: Value) -> Result<Download, String> {
        static USER_AGENT: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
        let agent = USER_AGENT
            .get_or_init(|| async {
                let version = async {
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .ok()?;
                    let value: Value = client
                        .get("https://appversion.115.com/1/web/1.0/api/chrome")
                        .send()
                        .await
                        .ok()?
                        .json()
                        .await
                        .ok()?;
                    let version = text(&value["data"]["win"]["version_code"]);
                    (!version.is_empty()
                        && version.len() <= 16
                        && version.chars().all(|c| c.is_ascii_digit() || c == '.'))
                    .then_some(version)
                }
                .await
                .unwrap_or_else(|| "36.0.1".into());
                format!("Mozilla/5.0 115Browser/{version}")
            })
            .await;
        let mut key = [0; 16];
        getrandom::fill(&mut key).map_err(|_| "无法生成 115 请求随机数")?;
        let body = crypto::encode115(&payload.to_string(), &key)?;
        let response = self
            .transport
            .client
            .post(self.endpoint(&format!("https://proapi.115.com{path}")))
            .query(&[("t", now().to_string())])
            .header("User-Agent", agent)
            .header("Referer", "https://115.com/")
            .form(&[("data", body)])
            .send()
            .await
            .map_err(|_| "连接 115 下载接口失败，请稍后重试")?;
        if !response.status().is_success() {
            return Err(format!(
                "115 下载接口返回 HTTP {}",
                response.status().as_u16()
            ));
        }
        let cookies = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|header| header.to_str().ok().map(String::from))
            .collect::<Vec<_>>();
        let value: Value = response.json().await.map_err(|_| "115 下载响应无效")?;
        let value = check(self.provider, value)?;
        let decoded = crypto::decode115(value["data"].as_str().ok_or("115 未返回下载信息")?, &key)?;
        let address =
            download_address(&decoded).ok_or("115 未返回文件直链，请检查会员权限和官方验证提示")?;
        let mut address = url::Url::parse(&address).map_err(|_| "115 下载地址无效")?;
        if address.scheme() == "http" {
            let _ = address.set_scheme("https");
        }
        let mut download = self.download_descriptor(address.to_string())?;
        download.user_agent = agent.clone();
        // 115's CDN validates the login session as well as the object-scoped cookie.
        let login = self.pan115_login_cookie();
        if !login.is_empty() {
            download.headers.push(format!("Cookie: {login}"));
        }
        for cookie in cookies {
            let pair = cookie.split(';').next().unwrap_or_default().trim();
            let Some((name, _)) = pair.split_once('=') else {
                continue;
            };
            let path = cookie.split(';').map(str::trim).find_map(|part| {
                part.strip_prefix("path=")
                    .or_else(|| part.strip_prefix("Path="))
            });
            if name.len() == 32
                && name.bytes().all(|c| c.is_ascii_hexdigit())
                && path.is_some_and(|path| path != "/" && address.path().starts_with(path))
            {
                if let Some(header) = download
                    .headers
                    .iter_mut()
                    .find(|header| header.starts_with("Cookie: "))
                {
                    header.push_str(&format!("; {pair}"))
                } else {
                    download.headers.push(format!("Cookie: {pair}"))
                }
            }
        }
        Ok(download)
    }
    pub async fn pan115_personal_download(&self, metadata: &Value) -> Result<Download, String> {
        let pick = text(&metadata["pc"]);
        if pick.is_empty() {
            return Err("115 文件缺少下载标识，请刷新文件列表".into());
        }
        self.pan115_app_download("/app/chrome/downurl", json!({"pickcode":pick}))
            .await
    }
    fn pan115_login_cookie(&self) -> String {
        use reqwest::cookie::CookieStore;
        let mut selected = std::collections::BTreeMap::new();
        for origin in ["https://115.com/", "https://proapi.115.com/"] {
            if let Some(header) = self.transport.jar.cookies(&origin.parse().unwrap()) {
                if let Ok(value) = header.to_str() {
                    for pair in value.split(';').map(str::trim) {
                        if let Some((name, value)) = pair.split_once('=') {
                            if crate::pan115_session::COOKIE_NAMES.contains(&name) {
                                selected
                                    .entry(name.to_string())
                                    .or_insert_with(|| value.to_string());
                            }
                        }
                    }
                }
            }
        }
        selected
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
    pub async fn pan115_share_download(
        &self,
        share: &ShareLink,
        file: &ShareFile,
    ) -> Result<Download, String> {
        self.pan115_app_download(
            "/app/share/downurl",
            json!({"share_code":share.id,"receive_code":share.passcode,"file_id":file.id}),
        )
        .await
    }
}
fn download_address(value: &Value) -> Option<String> {
    if let Some(url) = value["url"]["url"]
        .as_str()
        .or_else(|| value["url"].as_str())
    {
        return Some(url.into());
    }
    value
        .as_object()?
        .values()
        .find_map(|entry| entry["url"]["url"].as_str().map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_cookie_preserves_encoding_and_all_login_fields() {
        let transport = crate::quark::Quark::new().unwrap();
        let origin = "https://115.com/".parse().unwrap();
        for name in crate::pan115_session::COOKIE_NAMES {
            transport.jar.add_cookie_str(
                &format!("{name}=encoded%2Fvalue%3D; Domain=.115.com; Path=/; Secure"),
                &origin,
            );
        }
        transport
            .jar
            .add_cookie_str("tracking=exclude; Path=/", &origin);
        let session = Session {
            provider: crate::model::Provider::Pan115,
            transport,
            authorization: String::new(),
            device_id: String::new(),
            test_endpoint: None,
        };
        let header = session.pan115_login_cookie();
        assert_eq!(header.split("; ").count(), 7);
        for name in crate::pan115_session::COOKIE_NAMES {
            assert!(header
                .split("; ")
                .any(|pair| pair == format!("{name}=encoded%2Fvalue%3D")));
        }
        assert!(!header.contains("tracking"));
    }
}
