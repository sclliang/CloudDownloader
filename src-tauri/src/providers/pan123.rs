use super::{
    cloud::{check, entry, Page},
    crypto, number, text, Session,
};
use crate::model::*;
use serde_json::{json, Value};

impl Session {
    async fn pan123_request(
        &self,
        path: &str,
        body: Option<Value>,
        query: Vec<(&str, String)>,
        share_download: bool,
    ) -> Result<Value, String> {
        let token = self.authorization.trim().trim_start_matches("Bearer ");
        if token.is_empty() || token.len() > 16384 || token.chars().any(char::is_control) {
            return Err("请先在官方窗口登录 123 云盘，再刷新登录状态".into());
        }
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
            .json(
                request
                    .query(&query)
                    .header("authorization", format!("Bearer {token}"))
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
            let value = check(self.provider, self.json(request).await?)?;
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
        let (path, body) = if let Some(share) = share {
            (
                "/b/api/share/download/info",
                json!({"ShareKey":share.id,"FileID":file.id,"S3KeyFlag":text(&metadata["S3KeyFlag"]),"Size":file.size,"Etag":text(&metadata["Etag"])}),
            )
        } else {
            (
                "/api/file/download_info",
                json!({"driveId":0,"etag":text(&metadata["Etag"]),"fileId":file.id.parse::<u64>().map_err(|_|"123 文件标识无效")?,"s3keyFlag":text(&metadata["S3KeyFlag"]),"type":0,"fileName":file.name,"size":file.size}),
            )
        };
        let value = self
            .pan123_request(path, Some(body), vec![], share.is_some())
            .await?;
        let address = value["data"]["DownloadURL"]
            .as_str()
            .or_else(|| value["data"]["DownloadUrl"].as_str())
            .ok_or("123 未返回下载链接，请检查下载权益与流量限制")?;
        let mut address = decode_download(address)?;
        // JSON redirect responses are resolved without credentials, only within CDN domains.
        for _ in 0..5 {
            address = decode_download(&address)?;
            let url = super::validate_download(self.provider, &address)?;
            if !url
                .query_pairs()
                .any(|(key, value)| key == "auto_redirect" && value == "0")
            {
                return Ok(address);
            }
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|_| "无法初始化 123 下载连接")?;
            let response = client
                .get(url)
                .header("Referer", "https://yun.123pan.cn/")
                .header("Range", "bytes=0-8191")
                .send()
                .await
                .map_err(|_| "123 下载地址检查失败")?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or("123 下载跳转缺少地址")?;
                address = response
                    .url()
                    .join(location)
                    .map_err(|_| "123 下载跳转地址无效")?
                    .to_string();
                continue;
            }
            if response
                .content_length()
                .is_some_and(|length| length <= 8192)
            {
                let bytes = response.bytes().await.map_err(|_| "123 下载地址响应无效")?;
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(next) = value["data"]["redirect_url"].as_str() {
                        address = next.into();
                        continue;
                    }
                }
            }
            return Ok(address);
        }
        Err("123 下载跳转次数过多，请稍后重试".into())
    }
}
fn next_cursor(data: &Value, length: usize) -> Option<String> {
    let next = text(&data["Next"]);
    (length > 0 && next != "-1" && (data.get("Next").is_some() || length == 100)).then_some(next)
}
pub fn decode_download(value: &str) -> Result<String, String> {
    let decoded = if let Ok(url) = url::Url::parse(value) {
        if let Some((_, parameters)) = url.query_pairs().find(|(key, _)| key == "params") {
            String::from_utf8(crypto::base64_decode(&parameters)?)
                .map_err(|_| "123 下载地址编码无效")?
        } else {
            value.into()
        }
    } else {
        String::from_utf8(crypto::base64_decode(value)?).map_err(|_| "123 下载地址编码无效")?
    };
    let mut url = url::Url::parse(&decoded).map_err(|_| "123 下载地址无效")?;
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    Ok(url.to_string())
}
