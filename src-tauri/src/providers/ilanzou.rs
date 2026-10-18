// Wire fields and endpoints: CYQawa/YunX (see cloud-protocol-sources.txt).
use super::{
    cloud::{identifier, Entry, Page},
    number, text, Session,
};
use crate::model::*;
use serde_json::Value;
const API: &str = "https://apis.ilanzou.com";
const SHARE_API: &str = "https://apix.ilanzou.com";

pub(super) fn aes_hex(plain: &str) -> String {
    use aes::cipher::{BlockEncrypt, KeyInit};
    let cipher = aes::Aes128::new(b"lanZouY-disk-app".into());
    let mut bytes = plain.as_bytes().to_vec();
    let padding = 16 - bytes.len() % 16;
    bytes.resize(bytes.len() + padding, padding as u8);
    for block in bytes.chunks_exact_mut(16) {
        cipher.encrypt_block(block.into());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn numeric_id(value: &str) -> Result<&str, String> {
    let id = value
        .strip_prefix("d_")
        .or_else(|| value.strip_prefix("f_"))
        .unwrap_or(value);
    if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("蓝奏文件标识无效，请刷新列表".into());
    }
    Ok(id)
}
fn file(raw: &Value, parent: &str) -> Result<Entry, String> {
    let dir = number(&raw["fileType"]) == 2;
    let id = text(&raw[if dir { "folderId" } else { "fileId" }]);
    numeric_id(&id)?;
    if id == "0" {
        return Err("蓝奏优享文件标识无效".into());
    }
    let key = format!("{}_{id}", if dir { "d" } else { "f" });
    let name = text(&raw[if dir { "folderName" } else { "fileName" }]);
    if name.is_empty() {
        return Err("蓝奏优享文件名称缺失".into());
    }
    Ok(Entry {
        file: DriveFile {
            id: key.clone(),
            path: format!("{}/{key}", parent.trim_end_matches('/')),
            name,
            size: if dir {
                0
            } else {
                number(&raw["fileSize"])
                    .checked_mul(1024)
                    .ok_or("文件大小异常")?
            },
            is_dir: dir,
            modified: 0,
            md5: String::new(),
        },
        metadata: raw.clone(),
    })
}
impl Session {
    fn ilanzou_request(&self, path: &str, share: bool, extra: bool) -> reqwest::RequestBuilder {
        let base = if share { SHARE_API } else { API };
        let timestamp = aes_hex(&chrono::Utc::now().timestamp_millis().to_string());
        let mut request = self
            .transport
            .client
            .get(self.endpoint(&format!("{base}{path}")))
            .header("Origin", "https://www.ilanzou.com")
            .header("Referer", "https://www.ilanzou.com/")
            .header("Accept", "application/json, text/plain, */*")
            .header("sec-ch-ua", r#""Chromium";v="130", "Not_A Brand";v="24""#)
            .header("sec-ch-ua-mobile", "?0")
            .header("sec-ch-ua-platform", r#""Windows""#)
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site")
            .query(&[
                ("uuid", self.device_id.as_str()),
                ("devCode", self.device_id.as_str()),
                ("devType", "6"),
                ("devModel", if share { "Chrome" } else { "chrome" }),
                ("devVersion", "130"),
                ("appVersion", ""),
                ("timestamp", timestamp.as_str()),
            ]);
        if !share && !self.authorization.is_empty() {
            request = request.query(&[("appToken", &self.authorization)]);
        }
        if extra {
            request = request.query(&[("extra", "2")]);
        }
        request
    }
    async fn ilanzou_json(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let mut request = request.build().map_err(|_| "蓝奏优享请求参数无效")?;
        if let Some(query) = request.url().query().map(|s| s.replace("%3A", ":")) {
            request.url_mut().set_query(Some(&query));
        }
        let response = self
            .transport
            .client
            .execute(request)
            .await
            .map_err(|_| "连接蓝奏云优享版失败，请检查网络")?;
        let value: Value = serde_json::from_str(&super::response_text(response).await?)
            .map_err(|_| "蓝奏云优享版返回了验证页面，请在官方窗口检查")?;
        if number(&value["code"]) != 200 {
            let code = value["code"].as_i64().unwrap_or(-99);
            return Err(if matches!(code, -1 | -2) {
                "蓝奏云优享版登录已失效，请重新登录".into()
            } else {
                format!("蓝奏云优享版请求失败（错误码 {code}），请检查链接、提取码或官方验证提示")
            });
        }
        Ok(value)
    }
    pub(crate) async fn ilanzou_uuid(&self) -> Result<String, String> {
        let value = self
            .ilanzou_json(self.ilanzou_request("/unproved/getUuid", false, true))
            .await?;
        let id = text(&value["uuid"]);
        identifier(&id)?;
        if id.len() < 8 {
            return Err("蓝奏优享设备标识无效".into());
        }
        Ok(id)
    }
    pub(super) async fn ilanzou_identity(&self) -> Result<(String, String), String> {
        if self.authorization.is_empty() {
            return Err("请先在官方窗口登录蓝奏云优享版".into());
        }
        let value = self
            .ilanzou_json(self.ilanzou_request("/proved/user/account/map", false, true))
            .await?;
        let map = &value["map"];
        let id = text(&map["userId"]);
        numeric_id(&id)?;
        let name = ["nickName", "nickname", "account"]
            .iter()
            .map(|key| text(&map[*key]))
            .find(|s| !s.is_empty())
            .unwrap_or_else(|| "蓝奏优享账号".into());
        Ok((id, name))
    }
    pub(super) async fn ilanzou_page(
        &self,
        path: &str,
        parent: &str,
        page: u32,
    ) -> Result<Page, String> {
        let value = self
            .ilanzou_json(
                self.ilanzou_request("/proved/record/file/list", false, true)
                    .query(&[
                        ("offset", page.to_string()),
                        ("limit", "60".into()),
                        ("folderId", numeric_id(parent)?.into()),
                        ("type", "0".into()),
                    ]),
            )
            .await?;
        let rows = value["list"].as_array().ok_or("蓝奏优享文件列表无效")?;
        let entries = rows
            .iter()
            .map(|raw| file(raw, path))
            .collect::<Result<Vec<_>, _>>()?;
        let total = number(&value["totalPage"]);
        Ok(Page {
            entries,
            next: ((total > 0 && (page as u64) < total) || (total == 0 && rows.len() == 60))
                .then(|| page.to_string()),
        })
    }
    pub(super) async fn ilanzou_share(
        &self,
        share: &ShareLink,
        parent: &str,
    ) -> Result<ShareListing, String> {
        identifier(&share.id)?;
        let mut files = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for page in 1..=200 {
            let mut request = self
                .ilanzou_request("/unproved/share/list", true, true)
                .query(&[
                    ("shareId", &share.id),
                    ("offset", &page.to_string()),
                    ("limit", &"60".into()),
                ]);
            if parent != "0" {
                request = request.query(&[("folderId", numeric_id(parent)?)]);
            }
            let value = self.ilanzou_json(request).await?;
            let rows = value["list"].as_array().ok_or("蓝奏优享分享列表无效")?;
            for raw in rows {
                let f = file(raw, "/")?.file;
                if !seen.insert(f.id.clone()) {
                    return Err("蓝奏优享分享分页重复，请重新解析".into());
                }
                files.push(ShareFile {
                    id: f.id,
                    name: f.name,
                    size: f.size,
                    is_dir: f.is_dir,
                    parent_id: parent.into(),
                    token: String::new(),
                });
            }
            let total = number(&value["totalPage"]);
            if (total > 0 && page as u64 >= total) || (total == 0 && rows.len() < 60) {
                return Ok(ShareListing {
                    share: share.clone(),
                    title: "蓝奏云优享版分享".into(),
                    parent_id: parent.into(),
                    stoken: String::new(),
                    files,
                });
            }
        }
        Err("蓝奏优享分享超过分页上限，请进入子目录".into())
    }
    pub(super) async fn ilanzou_download(
        &self,
        id: &str,
        share: Option<&ShareLink>,
    ) -> Result<String, String> {
        let fid = numeric_id(id)?;
        let user = if share.is_some() && self.authorization.is_empty() {
            String::new()
        } else if share.is_some() {
            self.ilanzou_identity()
                .await
                .map(|identity| identity.0)
                .unwrap_or_default()
        } else {
            self.ilanzou_identity().await?.0
        };
        let ts = chrono::Utc::now().timestamp_millis().to_string();
        let base = if share.is_some() { SHARE_API } else { API };
        let mut query = vec![
            ("uuid", self.device_id.clone()),
            ("devCode", self.device_id.clone()),
            ("devType", "6".into()),
            ("devModel", "Chrome".into()),
            ("devVersion", "130".into()),
            ("appVersion", String::new()),
            ("enable", "1".into()),
            ("timestamp", aes_hex(&ts)),
            ("downloadId", aes_hex(&format!("{fid}|{user}"))),
            ("auth", aes_hex(&format!("{fid}|{ts}"))),
        ];
        if let Some(share) = share {
            query.push(("shareId", share.id.clone()));
        } else {
            query.push(("appToken", self.authorization.clone()));
        }
        let client = self.no_redirect()?;
        let request = client
            .get(self.endpoint(&format!("{base}/unproved/file/redirect")))
            .query(&query)
            .header("Referer", "https://www.ilanzou.com/")
            .header("Origin", "https://www.ilanzou.com");
        let mut request = request.build().map_err(|_| "蓝奏优享下载参数无效")?;
        if let Some(query) = request.url().query().map(|s| s.replace("%3A", ":")) {
            request.url_mut().set_query(Some(&query));
        }
        let response = client
            .execute(request)
            .await
            .map_err(|_| "获取蓝奏优享下载地址失败，请检查网络")?;
        let url = if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|h| h.to_str().ok())
                .ok_or("蓝奏优享未返回下载跳转地址")?;
            url::Url::parse(base)
                .unwrap()
                .join(location)
                .map_err(|_| "蓝奏优享下载地址无效")?
                .to_string()
        } else {
            let value: Value = serde_json::from_str(&super::response_text(response).await?)
                .map_err(|_| "蓝奏优享返回了验证页面，请在官方窗口检查")?;
            if value["code"].as_i64() != Some(200) {
                return Err("蓝奏优享未返回下载地址，请检查文件权限或会员提示".into());
            }
            [text(&value["url"]), text(&value["data"]["url"])]
                .into_iter()
                .find(|v| !v.is_empty())
                .ok_or("蓝奏优享下载地址为空")?
        };
        super::validate_download(Provider::Ilanzou, &url)?;
        Ok(url)
    }
}
