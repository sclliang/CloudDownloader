// Classic Lanzou protocol, independently implemented from the documented wire
// flow in CYQawa/YunX. Account passwords stay in the official login WebView.
use super::{
    cloud::{Entry, Page},
    ilanzou::numeric_id,
    number, response_text, text, Download, Session,
};
use crate::model::*;
use serde_json::Value;
const PC: &str = "https://pc.woozooo.com";

fn capture(source: &str, pattern: &str) -> Option<String> {
    regex::Regex::new(pattern)
        .ok()?
        .captures(source)?
        .get(1)
        .map(|m| m.as_str().to_string())
}
fn js(source: &str, key: &str) -> Option<String> {
    capture(
        source,
        &format!(r#"(?:\b|['"]){key}['"]?\s*[:=]\s*['"]([^'"\r\n]*)['"]"#),
    )
    .or_else(|| capture(source, &format!(r"\b{key}\s*=\s*([0-9]+)\b")))
}
fn visible(source: &str) -> String {
    let mut s = regex::Regex::new(r"<[^>]*>")
        .unwrap()
        .replace_all(source, "")
        .into_owned();
    for (a, b) in [
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&nbsp;", " "),
    ] {
        s = s.replace(a, b);
    }
    s.trim().to_string()
}
fn title(html: &str) -> String {
    js(html, "filename")
        .or_else(|| capture(html, r#"class=['"]n['"][^>]*>([^<]+)"#))
        .or_else(|| capture(html, r"(?is)<title>(.*?)</title>"))
        .map(|s| visible(&s).trim_end_matches(" - 蓝奏云").trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains("密码") && s != "蓝奏云")
        .unwrap_or_else(|| "蓝奏云文件".into())
}
pub(super) fn display_size(raw: &str) -> u64 {
    let Some(c) = regex::Regex::new(r"(?i)^\s*([0-9]+(?:\.[0-9]+)?)\s*([KMGT]?)(?:i?B)?\s*$")
        .unwrap()
        .captures(raw)
    else {
        return 0;
    };
    let value = c[1].parse::<f64>().unwrap_or(0.0);
    let power = match c[2].to_ascii_uppercase().as_str() {
        "K" => 1,
        "M" => 2,
        "G" => 3,
        "T" => 4,
        _ => 0,
    };
    (value * 1024_f64.powi(power)) as u64
}
pub(super) fn challenge_cookie(html: &str) -> Option<String> {
    let arg = capture(html, r#"\barg1\s*=\s*['"]([0-9A-Fa-f]{40})['"]"#)?;
    let order = [
        15, 35, 29, 24, 33, 16, 1, 38, 10, 9, 19, 31, 40, 27, 22, 23, 25, 13, 6, 11, 39, 18, 20, 8,
        14, 21, 32, 26, 2, 30, 7, 4, 17, 5, 3, 28, 34, 37, 12, 36,
    ];
    let reordered: String = order
        .iter()
        .map(|n| arg.as_bytes()[n - 1] as char)
        .collect();
    let mask = "3000176000856006061501533003690027800375";
    (0..40)
        .step_by(2)
        .map(|i| {
            Some(format!(
                "{:02x}",
                u8::from_str_radix(&reordered[i..i + 2], 16).ok()?
                    ^ u8::from_str_radix(&mask[i..i + 2], 16).ok()?
            ))
        })
        .collect::<Option<String>>()
}
fn safe_page(address: &str) -> Result<url::Url, String> {
    let mut url = url::Url::parse(address).map_err(|_| "蓝奏页面地址无效")?;
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !(lanzou_host(url.host_str().unwrap_or_default())
            || host_matches(url.host_str().unwrap_or_default(), "woozooo.com"))
    {
        return Err("蓝奏页面跳转到了不支持的域名".into());
    }
    Ok(url)
}
fn node_url(address: &str) -> Result<url::Url, String> {
    let mut url = url::Url::parse(address).map_err(|_| "蓝奏下载地址无效")?;
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    super::validate_download(Provider::Lanzou, url.as_str())
}
impl Session {
    async fn lanzou_text(
        &self,
        address: &str,
        referer: &str,
        form: Option<&[(&str, String)]>,
    ) -> Result<(String, url::Url), String> {
        let client = self.no_redirect()?;
        let mut url = safe_page(address)?;
        let mut challenges = std::collections::HashSet::new();
        for _ in 0..8 {
            let mut request = if let Some(form) = form {
                client.post(self.endpoint(url.as_str())).form(form)
            } else {
                client.get(self.endpoint(url.as_str()))
            };
            request = request
                .header("Referer", referer)
                .header("Origin", url.origin().ascii_serialization());
            let response = request
                .send()
                .await
                .map_err(|_| "连接蓝奏云失败，请检查网络")?;
            if response.status().is_redirection() {
                if form.is_some() {
                    return Err("蓝奏会话需要重新验证，请在官方窗口检查".into());
                }
                let target = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or("蓝奏页面跳转无效")?;
                url = safe_page(url.join(target).map_err(|_| "蓝奏页面地址无效")?.as_str())?;
                continue;
            }
            if !response.status().is_success() {
                return Err(format!(
                    "蓝奏云请求失败（HTTP {}），请在官方窗口检查验证提示",
                    response.status().as_u16()
                ));
            }
            let body = response_text(response).await?;
            if let Some(cookie) = challenge_cookie(&body) {
                if !challenges.insert(url.host_str().unwrap_or_default().to_string()) {
                    return Err("蓝奏访问验证未通过，请在官方窗口完成验证后重试".into());
                }
                self.transport
                    .jar
                    .add_cookie_str(&format!("acw_sc__v2={cookie}; Path=/; Secure"), &url);
                continue;
            }
            return Ok((body, url));
        }
        Err("蓝奏页面跳转或验证次数过多，请稍后重试".into())
    }
    async fn lanzou_login_params(&self) -> Result<(String, String), String> {
        let (html, _) = self
            .lanzou_text(
                &format!("{PC}/mydisk.php?item=files&action=index"),
                &format!("{PC}/mydisk.php"),
                None,
            )
            .await?;
        let uid = capture(&html, r#"\buid['"]?\s*[=:]\s*['"]?(\d{1,20})"#)
            .ok_or("请先在官方窗口登录蓝奏云")?;
        let vei = capture(
            &html,
            r#"\bvei['"]?\s*[=:]\s*['"]?([A-Za-z0-9_+/=\-]{1,256})"#,
        )
        .ok_or("蓝奏登录会话无效，请重新登录")?;
        Ok((uid, vei))
    }
    async fn lanzou_api(
        &self,
        uid: &str,
        vei: &str,
        fields: &[(&str, String)],
    ) -> Result<Value, String> {
        let mut url = url::Url::parse(&format!("{PC}/doupload.php")).unwrap();
        url.query_pairs_mut()
            .append_pair("uid", uid)
            .append_pair("vei", vei);
        let (body, _) = self
            .lanzou_text(url.as_str(), &format!("{PC}/mydisk.php"), Some(fields))
            .await?;
        let value: Value =
            serde_json::from_str(&body).map_err(|_| "蓝奏返回了验证页面，请打开官方窗口检查")?;
        if !matches!(number(&value["zt"]), 1 | 2) {
            return Err(format!(
                "蓝奏云请求未成功（错误码 {}），请检查登录、提取码或访问验证",
                number(&value["zt"])
            ));
        }
        Ok(value)
    }
    pub(super) async fn lanzou_identity(&self) -> Result<(String, String), String> {
        let (uid, _) = self.lanzou_login_params().await?;
        Ok((uid, "蓝奏云账号".into()))
    }
    pub(super) async fn lanzou_page(
        &self,
        path: &str,
        parent: &str,
        page: u32,
    ) -> Result<Page, String> {
        let folder = if parent == "0" {
            "-1"
        } else {
            numeric_id(parent)?
        };
        let (uid, vei) = self.lanzou_login_params().await?;
        let mut entries = Vec::new();
        if page == 1 {
            let dirs = self
                .lanzou_api(
                    &uid,
                    &vei,
                    &[("task", "47".into()), ("folder_id", folder.into())],
                )
                .await?;
            if let Some(rows) = dirs["text"].as_array() {
                for raw in rows {
                    let id = [text(&raw["fol_id"]), text(&raw["id"])]
                        .into_iter()
                        .find(|v| !v.is_empty())
                        .ok_or("蓝奏文件夹标识缺失")?;
                    numeric_id(&id)?;
                    let key = format!("d_{id}");
                    let name = [text(&raw["name"]), text(&raw["name_all"])]
                        .into_iter()
                        .find(|v| !v.is_empty())
                        .ok_or("蓝奏文件夹名称缺失")?;
                    entries.push(Entry {
                        file: DriveFile {
                            id: key.clone(),
                            path: format!("{}/{key}", path.trim_end_matches('/')),
                            name: visible(&name),
                            size: 0,
                            is_dir: true,
                            modified: 0,
                            md5: String::new(),
                        },
                        metadata: raw.clone(),
                    });
                }
            }
        }
        let value = self
            .lanzou_api(
                &uid,
                &vei,
                &[
                    ("task", "5".into()),
                    ("folder_id", folder.into()),
                    ("pg", page.to_string()),
                ],
            )
            .await?;
        let rows = value["text"].as_array();
        if let Some(rows) = rows {
            for raw in rows {
                let id = text(&raw["id"]);
                numeric_id(&id)?;
                let key = format!("f_{id}");
                let name = [text(&raw["name_all"]), text(&raw["name"])]
                    .into_iter()
                    .find(|v| !v.is_empty())
                    .ok_or("蓝奏文件名称缺失")?;
                entries.push(Entry {
                    file: DriveFile {
                        id: key.clone(),
                        path: format!("{}/{key}", path.trim_end_matches('/')),
                        name: visible(&name),
                        size: display_size(&text(&raw["size"])),
                        is_dir: false,
                        modified: 0,
                        md5: String::new(),
                    },
                    metadata: raw.clone(),
                });
            }
        }
        Ok(Page {
            entries,
            next: (number(&value["zt"]) == 1 && rows.is_some_and(|r| !r.is_empty()))
                .then(|| page.to_string()),
        })
    }
    pub(super) async fn lanzou_share(
        &self,
        share: &ShareLink,
        parent: &str,
    ) -> Result<ShareListing, String> {
        let initial = safe_page(&share.id)?;
        if !lanzou_host(initial.host_str().unwrap_or_default())
            || initial.path().trim_matches('/').is_empty()
        {
            return Err("蓝奏分享地址无效".into());
        }
        if parent != "0" {
            return Err("普通蓝奏文件夹分享不包含下一级目录".into());
        }
        let (html, url) = self
            .lanzou_text(initial.as_str(), initial.as_str(), None)
            .await?;
        if html.contains("文件已取消") || html.contains("文件不存在") {
            return Err("蓝奏分享已取消或文件不存在".into());
        }
        let folder = html.contains("filemoreajax") || html.contains("file_more");
        let mut files = Vec::new();
        if folder {
            let lx = js(&html, "lx").unwrap_or_else(|| "2".into());
            let fid = js(&html, "fid").ok_or("蓝奏分享页缺少目录参数")?;
            let t = js(&html, "t").ok_or("蓝奏分享页缺少分页参数")?;
            let k = js(&html, "k").ok_or("蓝奏分享页缺少分页签名")?;
            let ajax = url.join("/filemoreajax.php").unwrap();
            let mut seen = std::collections::HashSet::new();
            for page in 1..=200 {
                let (body, _) = self
                    .lanzou_text(
                        ajax.as_str(),
                        url.as_str(),
                        Some(&[
                            ("lx", lx.clone()),
                            ("fid", fid.clone()),
                            ("t", t.clone()),
                            ("k", k.clone()),
                            ("pwd", share.passcode.clone()),
                            ("pg", page.to_string()),
                        ]),
                    )
                    .await?;
                let value: Value =
                    serde_json::from_str(&body).map_err(|_| "蓝奏分享列表返回了验证页面")?;
                match number(&value["zt"]) {
                    2 => break,
                    1 => (),
                    3 => return Err("蓝奏提取码错误，请重新输入".into()),
                    _ => return Err("蓝奏分享解析失败，请检查提取码或稍后重试".into()),
                }
                let rows = value["text"].as_array().ok_or("蓝奏分享列表无效")?;
                if rows.is_empty() {
                    break;
                }
                for raw in rows {
                    let id = text(&raw["id"]);
                    super::cloud::identifier(&id)?;
                    if !seen.insert(id.clone()) {
                        return Err("蓝奏分享分页重复，请重新解析".into());
                    }
                    let name = [text(&raw["name_all"]), text(&raw["name"])]
                        .into_iter()
                        .find(|v| !v.is_empty())
                        .ok_or("蓝奏文件名称缺失")?;
                    let file_url = url
                        .join(&format!("/{id}"))
                        .map_err(|_| "蓝奏文件地址无效")?;
                    files.push(ShareFile {
                        id,
                        name: visible(&name),
                        size: display_size(&text(&raw["size"])),
                        is_dir: false,
                        parent_id: parent.into(),
                        token: file_url.to_string(),
                    });
                }
                if page == 200 {
                    return Err("蓝奏分享超过分页上限，请分批下载".into());
                }
            }
        } else {
            if share.passcode.is_empty()
                && (html.contains("请输入密码")
                    || html.contains("input_password")
                    || html.contains("pwbox"))
            {
                return Err("此蓝奏分享需要提取码，请填写后重新解析".into());
            }
            let id = url.path().trim_matches('/').to_string();
            super::cloud::identifier(&id)?;
            let size = capture(
                &html,
                r"(?:文件大小|大小)\s*[:：]\s*([0-9.]+\s*[KMGTkmgt]?(?:i?B)?)",
            )
            .map(|v| display_size(&v))
            .unwrap_or(0);
            files.push(ShareFile {
                id,
                name: title(&html),
                size,
                is_dir: false,
                parent_id: parent.into(),
                token: url.to_string(),
            });
        }
        Ok(ShareListing {
            share: share.clone(),
            title: if folder {
                "蓝奏云文件夹分享".into()
            } else {
                title(&html)
            },
            parent_id: parent.into(),
            stoken: String::new(),
            files,
        })
    }
    pub(super) async fn lanzou_download(
        &self,
        share: &ShareLink,
        file: &ShareFile,
    ) -> Result<Download, String> {
        let target = if file.token.is_empty() {
            share.id.as_str()
        } else {
            file.token.as_str()
        };
        let (mut html, url) = self.lanzou_text(target, target, None).await?;
        if let Some(iframe) = capture(&html, r#"(?i)<iframe[^>]+src=['"]([^'"]+)['"]"#) {
            let next = url.join(&iframe).map_err(|_| "蓝奏下载页地址无效")?;
            if next.origin() != url.origin() {
                return Err("蓝奏下载页来源不一致".into());
            }
            html = self.lanzou_text(next.as_str(), url.as_str(), None).await?.0;
        }
        let id = capture(&html, r"(?:ajaxm|ajaxfile)\.php\?file=(\d+)")
            .or_else(|| capture(&html, r"file=(\d+)"))
            .ok_or("蓝奏分享页缺少文件参数，请刷新重试")?;
        let sign = js(&html, "wp_sign")
            .or_else(|| js(&html, "sign"))
            .ok_or("蓝奏分享页缺少下载签名，请刷新重试")?;
        let mut fields = vec![("action", "downprocess".into()), ("sign", sign)];
        if !share.passcode.is_empty() {
            fields.extend([("p", share.passcode.clone()), ("kd", "1".into())]);
        } else {
            let key = js(&html, "ajaxdata")
                .or_else(|| js(&html, "websignkey"))
                .unwrap_or_default();
            fields.extend([
                ("signs", js(&html, "signs").unwrap_or_else(|| key.clone())),
                ("websignkey", key),
                ("websign", js(&html, "websign").unwrap_or_default()),
                (
                    "kd",
                    capture(&html, r"\bkdns\s*=\s*([01])").unwrap_or_else(|| "1".into()),
                ),
                ("ves", "1".into()),
            ]);
        }
        let ajax = js(&html, "domain1")
            .or_else(|| js(&html, "domain2"))
            .filter(|v| v.contains("/ajax"))
            .or_else(|| {
                capture(
                    &html,
                    r#"(https?://[^'"\s]+/(?:ajaxm|ajaxfile)\.php\?file=\d+)"#,
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "{}/{}.php?file={id}",
                    url.origin().ascii_serialization(),
                    if share.passcode.is_empty() {
                        "ajaxm"
                    } else {
                        "ajaxfile"
                    }
                )
            });
        let (body, _) = self.lanzou_text(&ajax, url.as_str(), Some(&fields)).await?;
        let value: Value = serde_json::from_str(&body).map_err(|_| "蓝奏取链接口返回了验证页面")?;
        if number(&value["zt"]) != 1 {
            return Err("蓝奏取链失败，请检查提取码或官方验证提示".into());
        }
        let domain = text(&value["dom"]);
        let relative = text(&value["url"]);
        if relative.is_empty() || relative.starts_with('/') || relative.contains("://") {
            return Err("蓝奏返回了无效下载参数".into());
        }
        let base = url::Url::parse(&domain).map_err(|_| "蓝奏下载节点无效")?;
        let jump = format!("{}/file/{relative}", base.origin().ascii_serialization());
        self.lanzou_node(&jump, url.origin().ascii_serialization().as_str())
            .await
    }
    async fn lanzou_node(&self, address: &str, referer: &str) -> Result<Download, String> {
        let client = self.no_redirect()?;
        let mut url = node_url(address)?;
        let mut challenged = std::collections::HashSet::new();
        let mut confirmed = std::collections::HashSet::new();
        for _ in 0..10 {
            self.transport
                .jar
                .add_cookie_str("down_ip=1; Path=/; Secure", &url);
            let response = client
                .get(self.endpoint(url.as_str()))
                .header("Range", "bytes=0-0")
                .header("Referer", referer)
                .send()
                .await
                .map_err(|_| "连接蓝奏下载节点失败")?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or("蓝奏下载跳转无效")?;
                url = node_url(url.join(location).map_err(|_| "蓝奏下载地址无效")?.as_str())?;
                continue;
            }
            if !response.status().is_success() {
                return Err(format!(
                    "蓝奏下载节点返回 HTTP {}，请重新解析或在官网检查",
                    response.status().as_u16()
                ));
            }
            if response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|s| s.contains("text/html"))
            {
                let html = response_text(response).await?;
                if let Some(cookie) = challenge_cookie(&html) {
                    if !challenged.insert(url.host_str().unwrap_or_default().to_string()) {
                        return Err("蓝奏下载节点需要进一步验证，请在官网完成后重试".into());
                    }
                    self.transport
                        .jar
                        .add_cookie_str(&format!("acw_sc__v2={cookie}; Path=/; Secure"), &url);
                    continue;
                }
                if html.contains("验证并下载")
                    && confirmed.insert(url.host_str().unwrap_or_default().to_string())
                {
                    let file = js(&html, "file").ok_or("蓝奏下载确认参数缺失，请在官网检查")?;
                    let sign = js(&html, "sign").ok_or("蓝奏下载确认签名缺失，请在官网检查")?;
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let ajax = url.join("ajax.php").unwrap();
                    super::validate_download(Provider::Lanzou, ajax.as_str())?;
                    let response = client
                        .post(self.endpoint(ajax.as_str()))
                        .header("Referer", url.as_str())
                        .form(&[
                            ("file", file.as_str()),
                            ("el", "2"),
                            ("sign", sign.as_str()),
                        ])
                        .send()
                        .await
                        .map_err(|_| "蓝奏下载确认失败")?;
                    let value: Value = serde_json::from_str(&response_text(response).await?)
                        .map_err(|_| "蓝奏下载确认返回了验证页面")?;
                    if number(&value["zt"]) != 1 {
                        return Err("蓝奏下载确认未通过，请在官网检查验证码".into());
                    }
                    url = node_url(&text(&value["url"]))?;
                    continue;
                }
                return Err("蓝奏返回了验证页面，请在官网完成验证后重试".into());
            }
            let size = response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next())
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    if response.status().as_u16() == 200 {
                        response.content_length()
                    } else {
                        None
                    }
                });
            let mut download = self.download_descriptor(url.to_string())?;
            download.headers[0] = format!("Referer: {referer}/");
            use reqwest::cookie::CookieStore;
            if let Some(cookie) = self
                .transport
                .jar
                .cookies(&url)
                .and_then(|v| v.to_str().ok().map(String::from))
            {
                let node_cookie = cookie
                    .split(';')
                    .map(str::trim)
                    .filter(|pair| {
                        pair.split_once('=').is_some_and(|(name, _)| {
                            matches!(name, "down_ip" | "acw_sc__v2" | "acw_tc")
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if !node_cookie.is_empty() {
                    download.headers.push(format!("Cookie: {node_cookie}"));
                }
            }
            download.size = size;
            return Ok(download);
        }
        Err("蓝奏下载跳转次数过多，请重新解析".into())
    }
    pub(super) async fn lanzou_personal_download(&self, id: &str) -> Result<Download, String> {
        if !id.starts_with("f_") {
            return Err("请选择蓝奏文件下载".into());
        }
        let (uid, vei) = self.lanzou_login_params().await?;
        // task=22 reads the file's existing external-link information. It does
        // not change passwords, sharing settings, or files in the account.
        let value = self
            .lanzou_api(
                &uid,
                &vei,
                &[("task", "22".into()), ("file_id", numeric_id(id)?.into())],
            )
            .await?;
        let info = &value["info"];
        let raw = text(&info["f_id"]);
        let domain = text(&info["is_newd"]);
        let base = if domain.is_empty() {
            "https://pan.lanzoui.com".into()
        } else if domain.starts_with("https://") {
            domain
        } else {
            format!("https://{domain}")
        };
        let url = if raw.starts_with("https://") {
            raw
        } else {
            format!("{}/{raw}", base.trim_end_matches('/'))
        };
        let share = super::parse_share(&url, &text(&info["pwd"]))?;
        let listing = self.lanzou_share(&share, "0").await?;
        let file = listing
            .files
            .first()
            .filter(|f| !f.is_dir)
            .ok_or("蓝奏文件外链已失效")?;
        self.lanzou_download(&share, file).await
    }
}
