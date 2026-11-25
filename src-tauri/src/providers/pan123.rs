use super::{
    cloud::{check, entry, Page},
    crypto, number, text, Session,
};
use crate::model::*;
use crate::network::{certificate_error, send_read_request};
use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_RANGE, LOCATION, RANGE, REFERER,
};
use serde_json::{json, Value};
use url::Url;

const MAX_REDIRECTS: usize = 5;
const MAX_REDIRECT_BODY: usize = 8192;
const MAX_DOWNLOAD_NODES: usize = 8;

pub(super) fn validate_node(url: Url) -> Result<Url, String> {
    let host = url.host_str().unwrap_or_default();
    // These CDN edge nodes also serve HTTPS on port 30443.
    let edge = host_matches(host, "pd1.cjjd19.com") || host_matches(host, "d.cjjd15.com");
    let domains = [
        "123pan.net",
        "123pancloud.com",
        "123pancloud.cn",
        "123pancloud.net",
        "123pan-cdn.com",
        "123952.com",
        "p123pan.com",
        "123pancdn.com",
    ];
    let reason = if url.scheme() != "https" {
        "下载节点需要 HTTPS"
    } else if !url.username().is_empty() || url.password().is_some() {
        "下载地址包含用户信息"
    } else if !edge
        && host != "download-cdn.cjjd19.com"
        && !PAN123_SHARE_DOMAINS
            .lines()
            .chain(domains)
            .any(|domain| host_matches(host, domain))
    {
        "尚未识别的下载域名"
    } else if url.port().is_some() && !(edge && url.port() == Some(30443)) {
        "不支持的下载端口"
    } else {
        return Ok(url);
    };
    Err(format!("123 云盘{reason}（{}）", node_name(&url)))
}

fn node_name(url: &Url) -> String {
    let host = url.host_str().unwrap_or("无主机名");
    match url.port_or_known_default() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

impl Session {
    fn pan123_authorization(&self) -> Result<String, String> {
        let token = self.authorization.trim().trim_start_matches("Bearer ");
        if token.is_empty() || token.len() > 16384 || token.chars().any(char::is_control) {
            return Err("请先在官方窗口登录 123 云盘，再刷新登录状态".into());
        }
        Ok(format!("Bearer {token}"))
    }

    async fn pan123_json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        self.json_response(send_read_request(request).await).await
    }

    async fn pan123_request(
        &self,
        path: &str,
        body: Option<Value>,
        query: Vec<(&str, String)>,
        share_download: bool,
    ) -> Result<Value, String> {
        let authorization = self.pan123_authorization()?;
        let (key, value) = crypto::sign123(
            path,
            now() as i64,
            u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap()) % 10000000,
        );
        let host = if share_download {
            "https://www.123865.com"
        } else {
            "https://yun.123pan.cn"
        };
        let request = if let Some(body) = body {
            self.transport
                .client
                .post(self.endpoint(&format!("{host}{path}")))
                .json(&body)
        } else {
            self.transport
                .client
                .get(self.endpoint(&format!("{host}{path}")))
        };
        let response = self
            .pan123_json(
                request
                    .query(&query)
                    .header("authorization", authorization)
                    .header("auth-key", key)
                    .header("auth-value", value)
                    .header("platform", if share_download { "android" } else { "web" })
                    .header("app-version", if share_download { "39" } else { "3" })
                    .header("loginuuid", &self.device_id)
                    .header("Referer", "https://yun.123pan.cn/"),
            )
            .await?;
        check(self.provider, response)
    }
    pub async fn pan123_identity(&self) -> Result<(String, String), String> {
        let value = self
            .pan123_request("/b/api/user/info", None, vec![], false)
            .await?;
        let data = &value["data"];
        let id = ["UID", "Uid", "UserId", "userId"]
            .iter()
            .map(|key| text(&data[*key]))
            .find(|id| !id.is_empty())
            .ok_or("123 未返回账号标识，请重新登录")?;
        let name = text(&data["Nickname"]);
        Ok((
            id,
            if name.is_empty() {
                "123 账号".into()
            } else {
                name
            },
        ))
    }
    pub async fn pan123_page(
        &self,
        path: &str,
        id: &str,
        page: u32,
        cursor: Option<&str>,
    ) -> Result<Page, String> {
        let value = self
            .pan123_request(
                "/b/api/file/list/new",
                None,
                vec![
                    ("driveId", "0".into()),
                    ("limit", "100".into()),
                    ("next", cursor.unwrap_or("0").into()),
                    ("orderBy", "update_time".into()),
                    ("orderDirection", "desc".into()),
                    ("parentFileId", id.into()),
                    ("trashed", "false".into()),
                    ("SearchData", "".into()),
                    ("Page", page.to_string()),
                    ("OnlyLookAbnormalFile", "0".into()),
                    ("event", "homeListFile".into()),
                    ("operateType", "1".into()),
                    ("inDirectSpace", "false".into()),
                ],
                false,
            )
            .await?;
        let rows = value["data"]["InfoList"]
            .as_array()
            .ok_or("123 文件列表无效")?;
        Ok(Page {
            entries: rows
                .iter()
                .map(|raw| entry(self.provider, raw, path))
                .collect::<Result<_, _>>()?,
            next: next_cursor(&value["data"], rows.len()),
        })
    }
    pub async fn pan123_share(
        &self,
        share: &ShareLink,
        parent: &str,
    ) -> Result<ShareListing, String> {
        super::cloud::identifier(parent)?;
        let mut files = Vec::new();
        let mut cursor = "0".to_string();
        for page in 1..=200 {
            let mut request = self
                .transport
                .client
                .get(self.endpoint("https://yun.123pan.cn/b/api/share/get"))
                .header("User-Agent", "Dart/3.12 (dart:io)")
                .query(&[
                    ("limit", "100".into()),
                    ("next", cursor.clone()),
                    ("orderBy", "file_name".into()),
                    ("orderDirection", "asc".into()),
                    ("shareKey", share.id.clone()),
                    ("ParentFileId", parent.into()),
                    ("Page", page.to_string()),
                ]);
            if !share.passcode.is_empty() {
                request = request.query(&[("SharePwd", &share.passcode)]);
            }
            let value = check(self.provider, self.pan123_json(request).await?)?;
            if value["data"]["Expired"].as_bool() == Some(true) {
                return Err("123 分享已失效".into());
            }
            let rows = value["data"]["InfoList"]
                .as_array()
                .ok_or("123 分享列表无效")?;
            for raw in rows {
                let id = text(&raw["FileId"]);
                super::cloud::identifier(&id)?;
                files.push(ShareFile {
                    id,
                    name: text(&raw["FileName"]),
                    size: number(&raw["Size"]),
                    is_dir: number(&raw["Type"]) == 1,
                    parent_id: parent.into(),
                    token: raw.to_string(),
                });
            }
            match next_cursor(&value["data"], rows.len()) {
                Some(next) => cursor = next,
                None => {
                    return Ok(ShareListing {
                        share: share.clone(),
                        title: "123 云盘分享".into(),
                        parent_id: parent.into(),
                        files,
                        stoken: String::new(),
                    })
                }
            }
        }
        Err("123 分享目录超过 20000 项，请分批下载".into())
    }
    pub async fn pan123_download(
        &self,
        file: &ShareFile,
        share: Option<&ShareLink>,
    ) -> Result<String, String> {
        let metadata: Value =
            serde_json::from_str(&file.token).map_err(|_| "123 文件信息已过期，请重新解析")?;
        let etag = text(&metadata["Etag"]);
        let (path, body) = if let Some(share) = share {
            (
                "/b/api/share/download/info",
                json!({"ShareKey":share.id,"FileID":file.id,"S3KeyFlag":text(&metadata["S3KeyFlag"]),"Size":file.size,"Etag":etag}),
            )
        } else {
            (
                "/api/file/download_info",
                json!({"driveId":0,"etag":etag,"fileId":file.id.parse::<u64>().map_err(|_|"123 文件标识无效")?,"s3keyFlag":text(&metadata["S3KeyFlag"]),"type":0,"fileName":file.name,"size":file.size}),
            )
        };
        let mut addresses = self.pan123_v2_download(&body, share.is_some()).await?;
        if addresses.is_empty() {
            let value = self
                .pan123_request(path, Some(body), vec![], share.is_some())
                .await?;
            addresses = download_addresses(&value);
        }
        let mut failure = "123 未返回下载链接，请检查下载权益与流量限制".to_string();
        for address in addresses {
            match self.resolve_pan123_download(&address, &etag).await {
                Ok(url) => return Ok(url),
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }

    async fn pan123_v2_download(&self, body: &Value, share: bool) -> Result<Vec<String>, String> {
        let mut body = body.clone();
        if share {
            if let Some(object) = body.as_object_mut() {
                if let Some(key) = object.remove("S3KeyFlag") {
                    object.insert("S3keyFlag".into(), key);
                }
                if let Some(id) = object.get("FileID").and_then(Value::as_str) {
                    let id = id.parse::<u64>().map_err(|_| "123 文件标识无效")?;
                    object.insert("FileID".into(), json!(id));
                }
            }
        }
        let path = if share {
            "/api/v2/share/download/info"
        } else {
            "/api/v2/file/download_info"
        };
        let response = send_read_request(
            self.no_redirect()?
                .post(self.endpoint(&format!("https://api.123pan.cn{path}")))
                .header("Authorization", self.pan123_authorization()?)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) 123pan/3.2.3 Chrome/108.0.5359.215 Electron/22.3.27 Safari/537.36 WebView/1.0")
                .header("Platform", "web")
                .header("Platform_os", "123pan/v323(Windows10.0.26200)")
                .header("App-Version", "323")
                .header("LoginUuid", &self.device_id)
                .header("Referer", "https://yun.123pan.cn/")
                .json(&body),
        ).await;
        let response = match response {
            Ok(response) => response,
            Err(error)
                if !certificate_error(&error) && (error.is_connect() || error.is_timeout()) =>
            {
                return Ok(Vec::new());
            }
            Err(_) => return Err("连接 123 新版下载接口失败".into()),
        };
        if matches!(response.status().as_u16(), 404 | 405 | 501)
            || response.status().is_server_error()
        {
            return Ok(Vec::new());
        }
        let status = response.status();
        let value = self.json_response(Ok(response)).await?;
        #[cfg(debug_assertions)]
        crate::verification::download_diagnostic(
            if share {
                "pan123-v2-share"
            } else {
                "pan123-v2-personal"
            },
            json!({"http":status.as_u16(),"hasCode":value.get("code").is_some(),
                "code":value["code"].as_i64(),
                "keys":value.as_object().map(|v|v.keys().collect::<Vec<_>>()),
                "dataKeys":value["data"].as_object().map(|v|v.keys().collect::<Vec<_>>()),
                "addressCount":download_addresses(&value).len()}),
        );
        // New desktop responses can omit code on success. Explicit business
        // errors must still stop here instead of falling back to another API.
        let value = if value.get("code").is_some_and(|code| !code.is_null()) {
            check(self.provider, value)?
        } else {
            value
        };
        if !status.is_success() {
            return Err(format!("123 新版下载接口返回 HTTP {}", status.as_u16()));
        }
        Ok(download_addresses(&value))
    }

    async fn resolve_pan123_download(&self, address: &str, etag: &str) -> Result<String, String> {
        // The API session carries account credentials. CDN requests only need the download headers.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(crate::quark::USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(12))
            .timeout(std::time::Duration::from_secs(25))
            .build()
            .map_err(|_| "无法初始化 123 下载连接")?;
        let mut url = decode_download(address, None)?;
        let range = format!("bytes=0-{}", MAX_REDIRECT_BODY - 1);
        for _ in 0..=MAX_REDIRECTS {
            url = validate_node(url)?;
            let mut response = self.get_pan123_node(&client, &url, Some(&range)).await?;
            if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                response = self.get_pan123_node(&client, &url, None).await?;
            }
            let status = response.status();
            let next = if status.is_redirection() {
                response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or("123 下载跳转缺少地址")?
                    .to_owned()
            } else if status.is_success() {
                match json_redirect(response, etag).await? {
                    Some(target) => target,
                    None => return Ok(url.into()),
                }
            } else {
                return Err(format!(
                    "123 云盘下载节点返回 HTTP {}（{}）",
                    status.as_u16(),
                    node_name(&url)
                ));
            };
            url = decode_download(&next, Some(&url))?;
        }
        Err("123 下载跳转次数过多，请稍后重试".into())
    }

    async fn get_pan123_node(
        &self,
        client: &reqwest::Client,
        url: &Url,
        range: Option<&str>,
    ) -> Result<reqwest::Response, String> {
        let mut request = client
            .get(self.endpoint(url.as_str()))
            .header(REFERER, "https://yun.123pan.cn/")
            .header(ACCEPT_ENCODING, "identity");
        if let Some(range) = range {
            request = request.header(RANGE, range);
        }
        send_read_request(request).await.map_err(|error| {
            let reason = if certificate_error(&error) {
                " TLS 证书验证失败"
            } else if error.is_timeout() {
                "连接超时"
            } else {
                "连接失败"
            };
            format!("123 云盘下载节点{reason}（{}）", node_name(url))
        })
    }
}

fn download_addresses(value: &Value) -> Vec<String> {
    let data = &value["data"];
    let mut addresses = Vec::new();
    for key in ["DownloadURL", "DownloadUrl"] {
        if let Some(address) = data[key]
            .as_str()
            .filter(|address| !address.trim().is_empty())
        {
            addresses.push(address.to_owned());
        }
    }
    if let (Some(nodes), Some(path)) = (
        data["dispatchList"].as_array(),
        data["downloadPath"].as_str(),
    ) {
        if !path.is_empty() {
            for node in nodes.iter().take(MAX_DOWNLOAD_NODES) {
                if let Some(prefix) = node["prefix"].as_str().filter(|prefix| !prefix.is_empty()) {
                    let address = format!("{prefix}{path}");
                    if !addresses.contains(&address) {
                        addresses.push(address);
                    }
                }
            }
        }
    }
    addresses.truncate(MAX_DOWNLOAD_NODES);
    addresses
}

async fn json_redirect(
    mut response: reqwest::Response,
    etag: &str,
) -> Result<Option<String>, String> {
    let attachment = response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("attachment"));
    let total_size = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once('/'))
        .and_then(|(_, size)| size.parse::<u64>().ok())
        .or_else(|| response.content_length());
    if attachment || total_size.is_some_and(|n| n > MAX_REDIRECT_BODY as u64) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "123 下载地址响应无效")? {
        if chunk.len() > MAX_REDIRECT_BODY - bytes.len() {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk);
        if bytes
            .iter()
            .find(|c| !c.is_ascii_whitespace())
            .is_some_and(|c| *c != b'{')
        {
            return Ok(None);
        }
    }
    let Ok(body) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Ok(None);
    };
    let Some(target) = value["data"]["redirect_url"].as_str() else {
        return Ok(None);
    };
    // A user's own JSON file can contain the same fields as a CDN redirect.
    if crypto::md5_hex(body).eq_ignore_ascii_case(etag) {
        return Ok(None);
    }
    Ok(Some(target.to_owned()))
}
fn next_cursor(data: &Value, length: usize) -> Option<String> {
    let next = text(&data["Next"]);
    (length > 0 && next != "-1" && (data.get("Next").is_some() || length == 100)).then_some(next)
}
pub(super) fn decode_download(value: &str, base: Option<&Url>) -> Result<Url, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("123 下载地址为空".into());
    }
    let mut url = Url::parse(value)
        .ok()
        .or_else(|| base64_url(value))
        .or_else(|| base?.join(value).ok())
        .ok_or("123 下载地址无效")?;
    // Some API versions use a different path for the same base64 URL wrapper.
    for _ in 0..2 {
        let Some((_, parameters)) = url.query_pairs().find(|(key, _)| key == "params") else {
            break;
        };
        match base64_url(&parameters.replace(' ', "+")) {
            Some(inner) => url = inner,
            None if url.path().trim_end_matches('/').ends_with("/download-v2") => {
                return Err("123 下载地址编码无效".into());
            }
            None => break,
        }
    }
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    Ok(url)
}

fn base64_url(value: &str) -> Option<Url> {
    let bytes = crypto::base64_decode(value).ok()?;
    let url = Url::parse(std::str::from_utf8(&bytes).ok()?).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}
