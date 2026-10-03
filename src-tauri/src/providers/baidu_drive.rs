use super::{number, text, Download, Session};
use crate::model::*;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use std::time::Duration;

pub struct Identity {
    pub account_id: String,
    pub token: String,
}

pub struct Precreate {
    pub upload_id: String,
    pub missing: Vec<usize>,
    pub complete: bool,
}

pub fn remote_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|p| matches!(p, "." | ".."))
        || path.len() > 4096
    {
        return Err("网盘路径无效".into());
    }
    Ok(())
}

fn check(data: Value) -> Result<Value, String> {
    let code = data["errno"]
        .as_i64()
        .or_else(|| data["error_code"].as_i64());
    if code == Some(0) {
        return Ok(data);
    }
    let description = match code {
        Some(-6 | -100 | 111 | 31045) => "登录已过期，请重新登录百度网盘",
        Some(-12 | -62 | 112 | 9019) => "需要网页验证，请打开百度网盘登录窗口完成验证",
        Some(-8 | -30 | 31061) => "目标位置已有同名文件，请修改文件名或选择其他目录",
        Some(-9 | 31066) => "文件或目录不存在，请刷新列表",
        Some(-10 | 31021) => "网盘剩余空间不足",
        Some(31034 | 31326) => "请求过于频繁，请稍后重试",
        Some(31363 | 31364 | 31079) => "上传会话已失效，请重试上传",
        Some(115 | 2161) => "百度限制了此文件的分享，请在官方窗口查看提示",
        _ => "请求未成功，请在百度网盘官方窗口检查权限或验证提示",
    };
    Err(format!(
        "百度网盘：{description}（错误码 {}）",
        code.map(|c| c.to_string()).unwrap_or_else(|| "缺失".into())
    ))
}

pub fn parse_file(v: &Value) -> Result<DriveFile, String> {
    let file = DriveFile {
        id: text(&v["fs_id"]),
        path: text(&v["path"]),
        name: text(&v["server_filename"]),
        size: number(&v["size"]),
        is_dir: number(&v["isdir"]) == 1,
        modified: number(&v["server_mtime"]),
        md5: text(&v["md5"]),
    };
    remote_path(&file.path)?;
    if file.id.parse::<u64>().is_err() {
        return Err("百度文件标识缺失".into());
    }
    Ok(DriveFile {
        name: if file.name.is_empty() {
            file.path.rsplit('/').next().unwrap_or("未命名").into()
        } else {
            file.name
        },
        ..file
    })
}

fn metadata_download_link<'a>(data: &'a Value, expected: &DriveFile) -> Result<&'a str, String> {
    let rows = data["info"].as_array().ok_or("百度文件信息不完整")?;
    if rows.len() != 1 {
        return Err("百度未返回唯一的文件信息，请刷新文件列表".into());
    }
    let fresh = parse_file(&rows[0])?;
    if fresh.is_dir
        || fresh.id != expected.id
        || fresh.path != expected.path
        || fresh.size != expected.size
        || (!expected.md5.is_empty() && fresh.md5 != expected.md5)
    {
        return Err("网盘源文件已变化，请刷新列表并创建新下载任务".into());
    }
    rows[0]["dlink"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("百度未返回下载地址，请在官方窗口检查下载权限".into())
}

impl Session {
    pub async fn drive_find(&self, path: &str) -> Result<Option<DriveFile>, String> {
        remote_path(path)?;
        let parent = path
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .ok_or("网盘路径无效")?;
        for page in 1..=200 {
            let listing = self.drive_list(parent, page, "").await?;
            if let Some(file) = listing.files.into_iter().find(|f| f.path == path) {
                return Ok(Some(file));
            }
            if !listing.has_more {
                return Ok(None);
            }
        }
        Err("目标目录超过 20000 项，请选择较小的目录上传".into())
    }
    fn drive_post(&self, path: &str, token: &str) -> reqwest::RequestBuilder {
        self.transport
            .client
            .post(format!("https://pan.baidu.com{path}"))
            .header("Referer", "https://pan.baidu.com/disk/main")
            .header("Origin", "https://pan.baidu.com")
            .query(&[
                ("channel", "chunlei"),
                ("clienttype", "0"),
                ("web", "1"),
                ("app_id", "250528"),
                ("bdstoken", token),
            ])
    }

    pub async fn baidu_identity(&self, expected: Option<&str>) -> Result<Identity, String> {
        if self.provider != Provider::Baidu {
            return Err("此操作仅支持百度网盘".into());
        }
        let data = check(
            self.json(
                self.baidu_get("/api/gettemplatevariable")
                    .query(&[("fields", r#"["uk","bdstoken","loginstate"]"#)]),
            )
            .await?,
        )?;
        let account_id = text(&data["result"]["uk"]);
        let token = text(&data["result"]["bdstoken"]);
        if account_id.is_empty()
            || account_id == "0"
            || token.is_empty()
            || data["result"]["loginstate"] == 0
        {
            return Err("请先登录百度网盘".into());
        }
        if expected.is_some_and(|v| v != account_id) {
            return Err("百度账号已切换，请切回创建任务时的账号，或刷新文件列表".into());
        }
        Ok(Identity { account_id, token })
    }

    pub async fn drive_list(
        &self,
        path: &str,
        page: u32,
        keyword: &str,
    ) -> Result<DriveListing, String> {
        remote_path(path)?;
        if page == 0 || page > 10000 || keyword.len() > 256 {
            return Err("分页或搜索参数无效".into());
        }
        let identity = self.baidu_identity(None).await?;
        let endpoint = if keyword.trim().is_empty() {
            "/api/list"
        } else {
            "/api/search"
        };
        let mut req = self
            .baidu_get(endpoint)
            .query(&[
                ("dir", path),
                ("order", "name"),
                ("desc", "0"),
                ("num", "100"),
                ("bdstoken", &identity.token),
            ])
            .query(&[("page", page)]);
        if !keyword.trim().is_empty() {
            req = req.query(&[("key", keyword.trim()), ("recursion", "1")]);
        }
        let data = check(self.json(req).await?)?;
        let rows = data["list"].as_array().ok_or("百度文件列表不完整")?;
        let files = rows.iter().map(parse_file).collect::<Result<Vec<_>, _>>()?;
        let has_more = data["has_more"].as_bool().unwrap_or_else(|| {
            data["has_more"]
                .as_u64()
                .map(|v| v != 0)
                .unwrap_or(rows.len() == 100)
        });
        Ok(DriveListing {
            account_id: identity.account_id,
            path: path.into(),
            page,
            has_more,
            files,
        })
    }

    pub async fn drive_meta(&self, paths: &[String]) -> Result<Vec<DriveFile>, String> {
        self.drive_meta_response(paths, false).await?["info"]
            .as_array()
            .ok_or("百度文件信息不完整")?
            .iter()
            .map(parse_file)
            .collect()
    }

    async fn drive_meta_response(&self, paths: &[String], dlink: bool) -> Result<Value, String> {
        if paths.is_empty() || paths.len() > 100 {
            return Err("一次最多选择 100 项".into());
        }
        for path in paths {
            remote_path(path)?;
        }
        check(
            self.json(
                self.baidu_get("/api/filemetas")
                    .header("Referer", "https://pan.baidu.com/disk/main")
                    .query(&[
                        ("target", json!(paths).to_string()),
                        ("dlink", if dlink { "1" } else { "0" }.into()),
                    ]),
            )
            .await?,
        )
    }

    pub async fn drive_download(&self, source: &OwnSource) -> Result<Download, String> {
        self.baidu_identity(Some(&source.account_id)).await?;
        // Fetch identity and a fresh link together. The legacy signed /api/download
        // route can return already-expired URLs for files in the personal drive.
        let data = self
            .drive_meta_response(std::slice::from_ref(&source.file.path), true)
            .await?;
        let link = metadata_download_link(&data, &source.file)?;
        self.download_descriptor(link.into())
    }

    pub async fn drive_mkdir(&self, identity: &Identity, path: &str) -> Result<DriveFile, String> {
        remote_path(path)?;
        let data = check(
            self.json(
                self.drive_post("/api/create", &identity.token)
                    .query(&[("a", "commit"), ("rtype", "0")])
                    .form(&[
                        ("path", path),
                        ("isdir", "1"),
                        ("size", "0"),
                        ("block_list", "[]"),
                        ("rtype", "0"),
                    ]),
            )
            .await?,
        )?;
        parse_file(&data)
    }

    pub async fn drive_share(
        &self,
        account_id: &str,
        files: &[DriveFile],
        period: u32,
        password: &str,
    ) -> Result<CreatedShare, String> {
        if !matches!(period, 0 | 1 | 7 | 30)
            || password.len() != 4
            || !password.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err("分享有效期请选择永久、1、7 或 30 天，提取码须为 4 位字母或数字".into());
        }
        let identity = self.baidu_identity(Some(account_id)).await?;
        let fresh = self
            .drive_meta(&files.iter().map(|f| f.path.clone()).collect::<Vec<_>>())
            .await?;
        if fresh.len() != files.len()
            || files
                .iter()
                .any(|f| !fresh.iter().any(|v| v.id == f.id && v.path == f.path))
        {
            return Err("文件列表已变化，请刷新后重新选择".into());
        }
        let ids = fresh
            .iter()
            .map(|f| f.id.parse::<u64>().map_err(|_| "文件 ID 无效"))
            .collect::<Result<Vec<_>, _>>()?;
        let data = check(
            self.json(self.drive_post("/share/set", &identity.token).form(&[
                ("fid_list", json!(ids).to_string()),
                ("schannel", "4".into()),
                ("channel_list", "[]".into()),
                ("period", period.to_string()),
                ("pwd", password.into()),
            ]))
            .await?,
        )?;
        let url = data["shorturl"]
            .as_str()
            .or_else(|| data["link"].as_str())
            .ok_or("百度未返回分享链接")?
            .replace("http://", "https://");
        let parsed = url::Url::parse(&url).map_err(|_| "分享链接无效")?;
        if parsed.scheme() != "https" || parsed.host_str() != Some("pan.baidu.com") {
            return Err("分享链接域名无效".into());
        }
        let id = text(&data["shareid"]);
        if id.is_empty() {
            return Err("分享标识缺失，请在官方窗口检查是否已创建".into());
        }
        Ok(CreatedShare {
            id,
            url,
            password: password.into(),
            period,
            created_at: now(),
            account_id: identity.account_id,
            names: fresh.iter().map(|f| f.name.clone()).collect(),
            cancelled: false,
        })
    }

    pub async fn drive_cancel_share(&self, share: &CreatedShare) -> Result<(), String> {
        let identity = self.baidu_identity(Some(&share.account_id)).await?;
        let id = share.id.parse::<u64>().map_err(|_| "分享 ID 无效")?;
        check(
            self.json(
                self.drive_post("/share/cancel", &identity.token)
                    .form(&[("shareid_list", json!([id]).to_string())]),
            )
            .await?,
        )?;
        Ok(())
    }

    pub async fn upload_precreate(
        &self,
        identity: &Identity,
        path: &str,
        size: u64,
        hashes: &[String],
        upload_id: &str,
    ) -> Result<Precreate, String> {
        remote_path(path)?;
        let mut body = vec![
            ("path", path.into()),
            ("size", size.to_string()),
            ("isdir", "0".into()),
            ("autoinit", "1".into()),
            ("rtype", "0".into()),
            ("block_list", json!(hashes).to_string()),
        ];
        if !upload_id.is_empty() {
            body.push(("uploadid", upload_id.into()));
        }
        let data = check(
            self.json(
                self.drive_post("/api/precreate", &identity.token)
                    .form(&body),
            )
            .await?,
        )?;
        let complete = number(&data["return_type"]) == 2;
        let missing = if complete {
            Vec::new()
        } else {
            data["block_list"]
                .as_array()
                .ok_or("百度未返回待上传分片列表")?
                .iter()
                .map(|n| {
                    n.as_u64()
                        .and_then(|i| usize::try_from(i).ok())
                        .filter(|i| *i < hashes.len())
                        .ok_or("百度返回了无效的分片编号".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        let upload_id = text(&data["uploadid"]);
        if !complete && upload_id.is_empty() {
            return Err("百度未返回上传会话".into());
        }
        Ok(Precreate {
            upload_id,
            missing,
            complete,
        })
    }

    pub async fn upload_part(
        &self,
        token: &str,
        path: &str,
        upload_id: &str,
        index: usize,
        bytes: Vec<u8>,
    ) -> Result<String, String> {
        let response = self
            .json(
                self.transport
                    .client
                    .post("https://d.pcs.baidu.com/rest/2.0/pcs/superfile2")
                    .timeout(Duration::from_secs(180))
                    .header("Referer", "https://pan.baidu.com/disk/main")
                    .query(&[
                        ("method", "upload"),
                        ("type", "tmpfile"),
                        ("app_id", "250528"),
                        ("channel", "chunlei"),
                        ("clienttype", "0"),
                        ("web", "1"),
                        ("bdstoken", token),
                        ("path", path),
                        ("uploadid", upload_id),
                    ])
                    .query(&[("partseq", index)])
                    .multipart(Form::new().part("file", Part::bytes(bytes).file_name("blob"))),
            )
            .await?;
        if response.get("error_code").is_some() || response.get("errno").is_some() {
            check(response.clone())?;
        }
        let md5 = text(&response["md5"]);
        if md5.len() != 32 {
            return Err("分片上传未得到有效确认，请重试".into());
        }
        Ok(md5)
    }

    pub async fn upload_finish(
        &self,
        identity: &Identity,
        path: &str,
        size: u64,
        hashes: &[String],
        upload_id: &str,
    ) -> Result<DriveFile, String> {
        let data = check(
            self.json(
                self.drive_post("/api/create", &identity.token)
                    .query(&[("a", "commit"), ("rtype", "0")])
                    .form(&[
                        ("path", path.into()),
                        ("size", size.to_string()),
                        ("isdir", "0".into()),
                        ("rtype", "0".into()),
                        ("block_list", json!(hashes).to_string()),
                        ("uploadid", upload_id.into()),
                    ]),
            )
            .await?,
        )?;
        parse_file(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_path_boundaries() {
        assert!(remote_path("/测试/文件.zip").is_ok());
        for path in ["relative", "/a/../b", "/a\\b", "/a\0b"] {
            assert!(remote_path(path).is_err());
        }
    }

    #[test]
    fn large_download_metadata_and_resume_identity() {
        let row = json!({"fs_id":123456789012345_u64, "path":"/test/large.7z",
            "size":7_348_134_959_u64, "isdir":0, "md5":"original",
            "dlink":"https://d.pcs.baidu.com/file?sign=private"});
        let expected = parse_file(&row).unwrap();
        assert_eq!(expected.size, 7_348_134_959);
        let data = json!({"info":[row.clone()]});
        assert_eq!(
            metadata_download_link(&data, &expected).unwrap(),
            row["dlink"]
        );
        for (field, changed) in [
            ("fs_id", json!(123)),
            ("path", json!("/different/large.7z")),
            ("size", json!(3_053_167_663_u64)),
            ("md5", json!("changed")),
            ("isdir", json!(1)),
            ("dlink", json!("")),
            ("dlink", Value::Null),
        ] {
            let mut data = data.clone();
            data["info"][0][field] = changed;
            assert!(metadata_download_link(&data, &expected).is_err(), "{field}");
        }
        for info in [json!([]), json!([row.clone(), row]), Value::Null] {
            assert!(metadata_download_link(&json!({"info":info}), &expected).is_err());
        }
    }
}
