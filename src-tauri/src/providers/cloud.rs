use super::{crypto, number, text, Download, Session};
use crate::model::*;
use reqwest::cookie::CookieStore;
use serde::Serialize;
use serde_json::{json, Value};

pub struct Entry {
    pub file: DriveFile,
    pub metadata: Value,
}
pub struct Page {
    pub entries: Vec<Entry>,
    pub next: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub account_id: String,
    pub path: String,
    pub files: Vec<DriveFile>,
    pub next_cursor: Option<String>,
}

pub fn check(provider: Provider, value: Value) -> Result<Value, String> {
    let valid = match provider {
        Provider::Quark => value["code"].as_i64() == Some(0),
        Provider::Pan115 => value["state"].as_bool() == Some(true),
        Provider::Pan123 => matches!(value["code"].as_i64(), Some(0 | 200)),
        Provider::C139 => {
            value["success"].as_bool() != Some(false)
                && ["0", "0000"].contains(
                    &text(if value.get("resultCode").is_some() {
                        &value["resultCode"]
                    } else {
                        &value["code"]
                    })
                    .as_str(),
                )
        }
        _ => false,
    };
    if valid {
        return Ok(value);
    }
    let code = ["errno", "errNo", "code", "resultCode"]
        .iter()
        .map(|key| text(&value[*key]))
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let code: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(24)
        .collect();
    if provider == Provider::Pan115 && code == "990001" {
        return Err("115 登录会话无效（错误码 990001），请在官方窗口重新登录后刷新登录状态".into());
    }
    Err(format!(
        "{}请求未成功（错误码 {}），请检查登录状态、提取码、会员权限或官方验证提示",
        provider.name(),
        code
    ))
}
pub fn identifier(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        || matches!(value, "." | "..")
    {
        return Err("网盘文件标识无效，请刷新列表".into());
    }
    Ok(value)
}
pub fn file_identifier(provider: Provider, value: &str) -> Result<&str, String> {
    // 139 system folders use IDs such as "/app-folder". Keep the provider ID
    // intact for API calls and encode its leading slash inside our UI path.
    let plain = if provider == Provider::C139 {
        value.strip_prefix('/').unwrap_or(value)
    } else {
        value
    };
    identifier(plain)?;
    Ok(value)
}
fn path_component(provider: Provider, value: &str) -> Result<String, String> {
    file_identifier(provider, value)?;
    Ok(if provider == Provider::C139 && value.starts_with('/') {
        format!("%2F{}", &value[1..])
    } else {
        value.into()
    })
}
fn component_id(provider: Provider, value: &str) -> Result<String, String> {
    let decoded =
        if provider == Provider::C139 && (value.starts_with("%2F") || value.starts_with("%2f")) {
            format!("/{}", &value[3..])
        } else {
            value.into()
        };
    file_identifier(provider, &decoded)?;
    Ok(decoded)
}
pub fn parent_id(path: &str, provider: Provider) -> Result<String, String> {
    if !path.starts_with('/') || path.len() > 8192 {
        return Err("网盘路径无效".into());
    }
    for part in path.split('/').filter(|part| !part.is_empty()) {
        component_id(provider, part)?;
    }
    let last = path.rsplit('/').find(|part| !part.is_empty()).unwrap_or("");
    if last.is_empty() {
        Ok(if provider == Provider::C139 { "/" } else { "0" }.into())
    } else {
        component_id(provider, last)
    }
}
pub fn entry(provider: Provider, raw: &Value, parent: &str) -> Result<Entry, String> {
    let (id, name, size, is_dir, modified, hash) = match provider {
        Provider::Quark => (
            text(&raw["fid"]),
            text(&raw["file_name"]),
            number(&raw["size"]),
            raw["dir"].as_bool().unwrap_or(false),
            number(&raw["updated_at"]) / 1000,
            text(&raw["md5"]),
        ),
        Provider::Pan115 => {
            let file = text(&raw["fid"]);
            let dir = file.is_empty();
            (
                if dir { text(&raw["cid"]) } else { file },
                text(&raw["n"]),
                number(&raw["s"]),
                dir,
                number(&raw["t"]),
                text(&raw["sha"]),
            )
        }
        Provider::Pan123 => (
            text(&raw["FileId"]),
            text(&raw["FileName"]),
            number(&raw["Size"]),
            number(&raw["Type"]) == 1,
            0,
            text(&raw["Etag"]),
        ),
        Provider::C139 => (
            text(&raw["fileId"]),
            text(&raw["name"]),
            number(&raw["size"]),
            raw["type"].as_str() == Some("folder"),
            0,
            text(&raw["md5"]),
        ),
        _ => return Err("此网盘使用专用文件接口".into()),
    };
    let component = path_component(provider, &id)?;
    if name.is_empty() {
        return Err("网盘文件名称缺失，请刷新列表".into());
    }
    Ok(Entry {
        file: DriveFile {
            id: id.clone(),
            path: format!("{}/{}", parent.trim_end_matches('/'), component),
            name,
            size,
            is_dir,
            modified,
            md5: hash,
        },
        metadata: raw.clone(),
    })
}
impl Session {
    pub fn cookie_value(&self, origins: &[&str], key: &str) -> Option<String> {
        origins
            .iter()
            .filter_map(|origin| origin.parse().ok())
            .filter_map(|url| self.transport.jar.cookies(&url))
            .filter_map(|header| header.to_str().ok().map(String::from))
            .flat_map(|value| {
                value
                    .split(';')
                    .map(str::trim)
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .find_map(|pair| {
                pair.split_once('=')
                    .filter(|(k, _)| *k == key)
                    .map(|(_, v)| {
                        percent_encoding::percent_decode_str(v)
                            .decode_utf8_lossy()
                            .into_owned()
                    })
            })
    }
    pub async fn cloud_identity(&self) -> Result<(String, String), String> {
        let (id, name) = match self.provider {
            Provider::Quark => {
                let value = self
                    .json(
                        self.transport
                            .client
                            .get(self.endpoint("https://pan.quark.cn/account/info"))
                            .query(&[("fr", "pc"), ("platform", "pc")])
                            .header("Referer", "https://pan.quark.cn/"),
                    )
                    .await?;
                if value["success"].as_bool() != Some(true) {
                    return Err("请先登录夸克网盘".into());
                }
                let data = &value["data"];
                let id = ["uid", "qid", "user_id", "userId", "ukey"]
                    .iter()
                    .map(|key| text(&data[*key]))
                    .find(|id| !id.is_empty())
                    .or_else(|| self.cookie_value(&["https://pan.quark.cn/"], "__uid"))
                    .ok_or("未取得夸克账号标识，请在官方窗口重新登录")?;
                (
                    id,
                    ["nickname", "nick_name", "nickName"]
                        .iter()
                        .map(|key| text(&data[*key]))
                        .find(|v| !v.is_empty())
                        .unwrap_or_else(|| "夸克账号".into()),
                )
            }
            Provider::Pan115 => self.pan115_identity().await?,
            Provider::Pan123 => self.pan123_identity().await?,
            Provider::C139 => self.c139_identity().await?,
            _ => return Err("此网盘使用专用文件接口".into()),
        };
        Ok((
            crypto::md5_hex(&format!("{}:{id}", self.provider.key())),
            name,
        ))
    }
    pub async fn cloud_page(
        &self,
        path: &str,
        page: u32,
        cursor: Option<&str>,
    ) -> Result<Page, String> {
        if page == 0 || page > 200 || cursor.is_some_and(|v| v.len() > 8192) {
            return Err("网盘分页参数无效".into());
        }
        let id = parent_id(path, self.provider)?;
        match self.provider {
            Provider::Quark => {
                let value = check(
                    self.provider,
                    self.json(
                        self.transport
                            .client
                            .get(self.endpoint("https://drive-pc.quark.cn/1/clouddrive/file/sort"))
                            .query(&[
                                ("pr", "ucpro".into()),
                                ("fr", "pc".into()),
                                ("pdir_fid", id),
                                ("_page", page.to_string()),
                                ("_size", "100".into()),
                                ("_fetch_total", "1".into()),
                                ("_sort", "file_type:asc,updated_at:desc".into()),
                            ])
                            .header("Referer", "https://pan.quark.cn/"),
                    )
                    .await?,
                )?;
                let rows = value["data"]["list"].as_array().ok_or("夸克文件列表无效")?;
                let entries = rows
                    .iter()
                    .map(|raw| entry(self.provider, raw, path))
                    .collect::<Result<Vec<_>, _>>()?;
                let more = rows.len() == 100
                    && (number(&value["metadata"]["_total"]) == 0
                        || number(&value["metadata"]["_total"]) > page as u64 * 100);
                Ok(Page {
                    entries,
                    next: more.then(|| page.to_string()),
                })
            }
            Provider::Pan115 => self.pan115_page(path, &id, page).await,
            Provider::Pan123 => self.pan123_page(path, &id, page, cursor).await,
            Provider::C139 => self.c139_page(path, &id, cursor).await,
            _ => Err("此网盘使用专用文件接口".into()),
        }
    }
    pub async fn cloud_find(&self, file: &DriveFile) -> Result<Entry, String> {
        file_identifier(self.provider, &file.id)?;
        if parent_id(&file.path, self.provider)? != file.id {
            return Err("文件标识与路径不一致，请刷新列表".into());
        }
        let parent = file
            .path
            .rsplit_once('/')
            .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
            .ok_or("文件路径无效")?;
        let mut cursor = None;
        for page in 1..=200 {
            let batch = self.cloud_page(parent, page, cursor.as_deref()).await?;
            if let Some(entry) = batch
                .entries
                .into_iter()
                .find(|entry| entry.file.id == file.id)
            {
                return Ok(entry);
            }
            cursor = batch.next;
            if cursor.is_none() {
                break;
            }
        }
        Err("网盘文件不存在或已被移动，请刷新列表".into())
    }
    pub async fn cloud_download(&self, source: &OwnSource) -> Result<Download, String> {
        if self.cloud_identity().await?.0 != source.account_id {
            return Err("网盘账号已切换，请使用创建任务时的账号继续下载".into());
        }
        let fresh = self.cloud_find(&source.file).await?;
        if fresh.file.is_dir
            || fresh.file.size != source.file.size
            || (!source.file.md5.is_empty() && fresh.file.md5 != source.file.md5)
        {
            return Err("源文件已变化，请刷新并新建下载任务".into());
        }
        let url = match self.provider {
            Provider::Quark => {
                let value =
                    check(
                        self.provider,
                        self.json(
                            self.transport
                                .client
                                .post(self.endpoint(
                                    "https://drive-pc.quark.cn/1/clouddrive/file/download",
                                ))
                                .query(&[
                                    ("pr", "ucpro"),
                                    ("fr", "pc"),
                                    ("sys", "win32"),
                                    ("ve", "6.9.7.761"),
                                ])
                                .header("Referer", "https://pan.quark.cn/")
                                .header("User-Agent", crate::quark::DOWNLOAD_AGENT)
                                .json(&json!({"fids":[fresh.file.id]})),
                        )
                        .await?,
                    )?;
                text(&value["data"][0]["download_url"])
            }
            Provider::Pan115 => return self.pan115_personal_download(&fresh.metadata).await,
            Provider::Pan123 => {
                let file = ShareFile {
                    id: fresh.file.id,
                    name: fresh.file.name,
                    size: fresh.file.size,
                    is_dir: false,
                    parent_id: String::new(),
                    token: fresh.metadata.to_string(),
                };
                self.pan123_download(&file, None).await?
            }
            Provider::C139 => self.c139_personal_download(&fresh.file.id).await?,
            _ => return Err("此网盘使用专用文件接口".into()),
        };
        self.download_descriptor(url)
    }
}
