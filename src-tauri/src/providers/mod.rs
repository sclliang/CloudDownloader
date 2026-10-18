mod baidu;
pub mod baidu_drive;
mod c139;
pub mod cloud;
mod crypto;
mod pan115;
mod pan123;
#[cfg(test)]
mod protocol_tests;

use crate::{model::*, quark::Quark};
use reqwest::cookie::CookieStore;
use serde_json::Value;

#[derive(Clone)]
pub struct Session {
    pub provider: Provider,
    pub transport: Quark,
    pub authorization: String,
    pub device_id: String,
    #[cfg(test)]
    pub test_endpoint: Option<String>,
}

pub struct Download {
    pub url: String,
    pub headers: Vec<String>,
    pub user_agent: String,
}

impl Session {
    fn endpoint(&self, address: &str) -> String {
        #[cfg(test)]
        if let Some(base) = &self.test_endpoint {
            let url = url::Url::parse(address).unwrap();
            return format!(
                "{base}{}{}",
                url.path(),
                url.query().map(|q| format!("?{q}")).unwrap_or_default()
            );
        }
        address.into()
    }
    pub async fn account(&self) -> Result<String, String> {
        match self.provider {
            Provider::Quark => self.transport.account().await,
            Provider::Baidu => self.baidu_account().await,
            _ => self.cloud_identity().await.map(|(_, name)| name),
        }
    }
    pub async fn list(&self, share: &ShareLink, parent: &str) -> Result<ShareListing, String> {
        if self.provider != share.provider {
            return Err("账号与分享来源不一致".into());
        }
        match self.provider {
            Provider::Quark => self.transport.list(share, parent).await,
            Provider::Baidu => self.baidu_list(share, parent).await,
            Provider::Pan115 => self.pan115_share(share, parent).await,
            Provider::Pan123 => self.pan123_share(share, parent).await,
            Provider::C139 => self.c139_share(share, parent).await,
        }
    }
    pub async fn download(
        &self,
        share: &ShareLink,
        file: &ShareFile,
        token: &str,
    ) -> Result<Download, String> {
        if self.provider != share.provider || file.is_dir {
            return Err("下载来源不一致或所选项目为文件夹，请刷新列表".into());
        }
        if self.provider == Provider::Pan115 {
            return self.pan115_share_download(share, file).await;
        }
        let url = match self.provider {
            Provider::Quark => self.transport.download(share, file, token).await?,
            Provider::Baidu => self.baidu_download(share, file, token).await?,
            Provider::Pan123 => self.pan123_download(file, Some(share)).await?,
            Provider::C139 => self.c139_share_download(share, file).await?,
            Provider::Pan115 => unreachable!(),
        };
        self.download_descriptor(url)
    }
    pub fn download_descriptor(&self, url: String) -> Result<Download, String> {
        let parsed = validate_download(self.provider, &url)?;
        let mut headers = vec![format!("Referer: {}/", self.provider.origin())];
        if let Some(cookies) = matches!(self.provider, Provider::Quark | Provider::Baidu)
            .then(|| {
                self.transport
                    .jar
                    .cookies(&parsed)
                    .and_then(|v| v.to_str().ok().map(String::from))
            })
            .flatten()
        {
            headers.push(format!("Cookie: {cookies}"));
        }
        Ok(Download {
            url,
            headers,
            user_agent: match self.provider {
                Provider::Quark => crate::quark::DOWNLOAD_AGENT,
                Provider::Baidu => "pan.baidu.com",
                _ => crate::quark::USER_AGENT,
            }
            .into(),
        })
    }
    pub async fn json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let response = request.send().await.map_err(|e| {
            format!(
                "连接{}{}",
                self.provider.name(),
                if e.is_timeout() {
                    "超时，请稍后重试"
                } else {
                    "失败，请检查网络"
                }
            )
        })?;
        let status = response.status();
        let value: Value = response.json().await.map_err(|_| {
            format!(
                "{}返回的内容无效（HTTP {}），请打开登录窗口检查验证提示",
                self.provider.name(),
                status.as_u16()
            )
        })?;
        if !status.is_success()
            && value.get("code").is_none()
            && value.get("errno").is_none()
            && value.get("res_code").is_none()
        {
            return Err(format!(
                "{}请求失败（HTTP {}）",
                self.provider.name(),
                status.as_u16()
            ));
        }
        Ok(value)
    }
}

pub fn text(value: &Value) -> String {
    value
        .as_str()
        .map(String::from)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .unwrap_or_default()
}
pub fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

pub fn parse_share(input: &str, code: &str) -> Result<ShareLink, String> {
    let pattern = regex::Regex::new(r#"https?://[^\s<>\"，。；）)\]]+"#).unwrap();
    let code_pattern =
        regex::Regex::new(r"(?:提取码|提取碼|密码|密碼|访问码)\s*[:：]?\s*([A-Za-z0-9]{4,8})\b")
            .unwrap();
    let mut found: Option<ShareLink> = None;
    for m in pattern.find_iter(input) {
        let Ok(url) = url::Url::parse(m.as_str()) else {
            continue;
        };
        if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
            continue;
        }
        let host = url.host_str().unwrap_or_default();
        let parts: Vec<_> = url.path().split('/').filter(|s| !s.is_empty()).collect();
        let provider = match host {
            "pan.quark.cn" => Provider::Quark,
            "pan.baidu.com" => Provider::Baidu,
            "115.com" | "www.115.com" | "115cdn.com" | "www.115cdn.com" | "115rc.com"
            | "www.115rc.com" | "anxia.com" => Provider::Pan115,
            "123pan.com" | "www.123pan.com" | "123pan.cn" | "www.123pan.cn" | "123865.com"
            | "www.123865.com" | "123912.com" | "www.123912.com" => Provider::Pan123,
            "yun.139.com" => Provider::C139,
            host if host_matches(host, "share.123pan.cn") => Provider::Pan123,
            _ => continue,
        };
        let mut embedded_code = String::new();
        let id: Option<String> = if provider == Provider::C139 {
            if url.path().trim_end_matches('/') != "/shareweb" {
                continue;
            }
            url.fragment()
                .and_then(|fragment| fragment.strip_prefix("/w/i/"))
                .map(|id| id.split(['?', '&']).next().unwrap_or_default().to_string())
        } else if provider == Provider::Pan123 && parts.as_slice() == ["api", "srr"] {
            url.query_pairs()
                .find(|(key, _)| key == "sk")
                .map(|(_, value)| value.into_owned())
        } else if parts.len() == 2
            && (parts[0] == "s"
                || (provider == Provider::Quark && parts[0] == "share")
                || (provider == Provider::Pan123 && parts[0] == "123pan"))
        {
            Some(parts[1].into())
        } else if provider == Provider::Pan115 && parts.len() == 1 {
            parts[0].rsplit_once('-').map(|(id, code)| {
                embedded_code = code.into();
                id.to_string()
            })
        } else {
            None
        };
        let Some(id) = id.filter(|v| {
            !v.is_empty()
                && v.len() <= 100
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }) else {
            continue;
        };
        let mut passcode = code.trim().to_string();
        if passcode.is_empty() {
            passcode = embedded_code;
        }
        if passcode.is_empty() {
            passcode = url
                .query_pairs()
                .find(|(k, _)| {
                    matches!(
                        k.as_ref(),
                        "pwd" | "passcode" | "password" | "accessCode" | "receive_code"
                    )
                })
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
        }
        if passcode.is_empty() {
            if let Some((_, query)) = url.fragment().and_then(|value| value.split_once('?')) {
                passcode = url::form_urlencoded::parse(query.as_bytes())
                    .find(|(key, _)| matches!(key.as_ref(), "pwd" | "password" | "passcode"))
                    .map(|(_, value)| value.into_owned())
                    .unwrap_or_default();
            }
        }
        if passcode.is_empty() {
            passcode = code_pattern
                .captures(input)
                .map(|c| c[1].into())
                .unwrap_or_default();
        }
        if !passcode.is_empty()
            && (!(4..=8).contains(&passcode.len())
                || !passcode.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Err("提取码应为 4 至 8 位字母或数字".into());
        }
        if let Some(previous) = &found {
            if previous.provider == provider && previous.id == id {
                continue;
            }
            return Err("请每次粘贴一个分享链接".into());
        }
        found = Some(ShareLink {
            provider,
            id,
            passcode,
        });
    }
    found.ok_or("请粘贴夸克、百度、115、123 或 139 的有效分享链接".into())
}

fn validate_download(provider: Provider, address: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(address).map_err(|_| "网盘返回了无效的下载地址")?;
    let domains: &[&str] = match provider {
        Provider::Quark => &[
            "quark.cn",
            "quark.com",
            "uc.cn",
            "ucweb.com",
            "aliyuncs.com",
        ],
        Provider::Baidu => &["baidu.com", "baidupcs.com", "bcebos.com", "baidubce.com"],
        Provider::Pan115 => &["115.com", "115cdn.com", "115cdn.net", "anxia.com"],
        Provider::Pan123 => &[
            "123pan.com",
            "123pan.cn",
            "123pan.net",
            "123pancloud.com",
            "123pancloud.cn",
            "123pancloud.net",
            "123pan-cdn.com",
            "123865.com",
            "123912.com",
            "123952.com",
            "123684.com",
            "p123pan.com",
            "123pancdn.com",
        ],
        Provider::C139 => &[
            "139.com",
            "10086.cn",
            "chinamobile.com",
            "migu.cn",
            "myhuaweicloud.com",
            "cmecloud.cn",
        ],
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !domains
            .iter()
            .any(|d| host_matches(url.host_str().unwrap_or_default(), d))
    {
        return Err(format!(
            "{}返回了尚未识别的下载域名，请更新适配器",
            provider.name()
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires an explicitly supplied live share; reads metadata only"]
    async fn live_share_listing() {
        let input = std::env::var("CLOUD_SMOKE_SHARE")
            .expect("set CLOUD_SMOKE_SHARE to a share URL and extraction code");
        let share = parse_share(&input, "").unwrap();
        let session = Session {
            provider: share.provider,
            transport: Quark::new().unwrap(),
            authorization: String::new(),
            device_id: String::new(),
            test_endpoint: None,
        };
        let listing = session.list(&share, "0").await.unwrap();
        assert!(
            !listing.files.is_empty(),
            "Expected a nonempty validation share"
        );
        for file in &listing.files {
            assert!(!file.id.is_empty());
            assert!(!file.name.is_empty());
        }
        println!(
            "{}: {} root entries",
            share.provider.name(),
            listing.files.len()
        );
        for file in listing.files.iter().filter(|f| f.is_dir).take(1) {
            let child = session.list(&share, &file.id).await.unwrap();
            assert!(
                !child.files.is_empty(),
                "Validation folder must not be empty"
            );
            println!(
                "{}: {} child entries",
                share.provider.name(),
                child.files.len()
            );
        }
    }
    #[test]
    fn markdown_link_deduplicates_and_secrets_are_not_serialized() {
        let share = parse_share(
            "[https://pan.quark.cn/s/one](https://pan.quark.cn/s/one)",
            "",
        )
        .unwrap();
        assert_eq!(share.id, "one");
        assert!(parse_share(
            "https://pan.quark.cn/s/one https://pan.baidu.com/s/1two",
            ""
        )
        .is_err());
        let file = ShareFile {
            id: "file".into(),
            name: "sample".into(),
            size: 1,
            is_dir: false,
            parent_id: "0".into(),
            token: "private-token".into(),
        };
        let listing = ShareListing {
            share,
            title: "sample".into(),
            parent_id: "0".into(),
            files: vec![file],
            stoken: "private-share-token".into(),
        };
        assert!(!serde_json::to_string(&listing).unwrap().contains("private"));
    }
    #[test]
    fn share_routes_and_legacy_default() {
        for (url, provider, id) in [
            (
                "https://pan.baidu.com/s/1abc?pwd=AB12",
                Provider::Baidu,
                "1abc",
            ),
            (
                "https://pan.quark.cn/s/74108d40cd8e",
                Provider::Quark,
                "74108d40cd8e",
            ),
        ] {
            let share = parse_share(&format!("链接：{url} 提取码：AB12"), "").unwrap();
            assert_eq!(share.provider, provider);
            assert_eq!(share.id, id);
            assert_eq!(share.passcode, "AB12");
        }
        let old: ShareLink = serde_json::from_str(r#"{"id":"old","passcode":"Ab12"}"#).unwrap();
        assert_eq!(old.provider, Provider::Quark);
        for bad in [
            "https://pan.baidu.com.evil.test/s/1x",
            "https://pan.quark.cn@evil.test/s/x",
            "https://pan.baidu.com:444/s/1x",
            "https://pan.quark.cn/s/x/extra",
            "https://www.alipan.com/s/unsupported",
        ] {
            assert!(parse_share(bad, "").is_err());
        }
    }
    #[test]
    fn download_host_boundary_and_no_cross_provider_credentials() {
        assert!(validate_download(Provider::Quark, "https://file.aliyuncs.com/file").is_ok());
        for url in [
            "http://file.aliyuncs.com/file",
            "https://evil.test/?url=aliyuncs.com",
            "https://aliyuncs.com.evil.test/file",
            "https://127.0.0.1/file",
            "https://pan.baidu.com/file",
        ] {
            assert!(validate_download(Provider::Quark, url).is_err());
        }
    }
}
