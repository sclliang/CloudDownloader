use super::{number, text, Session};
use crate::model::*;
use reqwest::cookie::CookieStore;
use serde_json::{json, Value};

pub(super) fn check(data: Value) -> Result<Value, String> {
    match data.get("errno").and_then(Value::as_i64) {
        Some(0) => Ok(data),
        Some(code) => {
            let message = match code {
                -9 => "提取码不正确",
                -6 | -100 | 111 => "请先登录百度网盘",
                -12 | -62 | 112 | 9019 => "需要网页验证，请打开百度网盘窗口完成验证后重试",
                -1 | 2 => "分享无效、已取消或参数已过期，请重新解析",
                118 | 119 => "下载权限不足，请在百度网盘官方页面检查账号或会员要求",
                _ => "请求未成功，请在百度网盘窗口检查提示",
            };
            Err(format!("百度网盘：{message}（错误码 {code}）"))
        }
        None => Err("百度网盘返回的数据不完整，请在官方窗口检查登录或验证提示".into()),
    }
}

impl Session {
    pub(super) fn baidu_get(&self, path: &str) -> reqwest::RequestBuilder {
        self.transport
            .client
            .get(format!("https://pan.baidu.com{path}"))
            .header("Referer", "https://pan.baidu.com/")
            .query(&[
                ("channel", "chunlei"),
                ("clienttype", "0"),
                ("web", "1"),
                ("app_id", "250528"),
            ])
    }
    pub async fn baidu_account(&self) -> Result<String, String> {
        let data = check(
            self.json(
                self.baidu_get("/api/gettemplatevariable")
                    .query(&[("fields", r#"["username","loginstate","bdstoken"]"#)]),
            )
            .await?,
        )?;
        let result = &data["result"];
        if text(&result["bdstoken"]).is_empty() || result["loginstate"] == 0 {
            return Err("请在百度网盘窗口完成登录".into());
        }
        Ok(result["username"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("百度网盘账号")
            .into())
    }
    pub async fn baidu_list(
        &self,
        share: &ShareLink,
        parent: &str,
    ) -> Result<ShareListing, String> {
        let short = share.id.strip_prefix('1').unwrap_or(&share.id);
        if !share.passcode.is_empty() {
            let req = self
                .transport
                .client
                .post("https://pan.baidu.com/share/verify")
                .query(&[
                    ("surl", short),
                    ("web", "1"),
                    ("channel", "chunlei"),
                    ("clienttype", "0"),
                ])
                .header("Referer", format!("https://pan.baidu.com/s/{}", share.id))
                .form(&[
                    ("pwd", share.passcode.as_str()),
                    ("vcode", ""),
                    ("vcode_str", ""),
                ]);
            check(self.json(req).await?)?;
            // reqwest retains the server BDCLND cookie verbatim; randsk is already URL-encoded.
        }
        let mut files = Vec::new();
        for page in 1..=200 {
            let mut req = self
                .baidu_get("/share/list")
                .query(&[
                    ("shorturl", short),
                    ("order", "name"),
                    ("desc", "0"),
                    ("num", "100"),
                ])
                .query(&[("page", page)]);
            req = if parent == "0" {
                req.query(&[("root", "1")])
            } else {
                req.query(&[("dir", parent)])
            };
            let data = check(self.json(req).await?)?;
            let batch = data["list"].as_array().ok_or("百度分享文件列表无效")?;
            for item in batch {
                let dir = number(&item["isdir"]) == 1;
                let id = text(&item[if dir { "path" } else { "fs_id" }]);
                if id.is_empty() {
                    return Err("百度文件标识缺失".into());
                }
                files.push(ShareFile {
                    id,
                    name: item["server_filename"]
                        .as_str()
                        .unwrap_or("未命名文件")
                        .into(),
                    size: number(&item["size"]),
                    is_dir: dir,
                    parent_id: parent.into(),
                    token: String::new(),
                });
            }
            if batch.len() < 100 {
                let metadata = json!({"uk":data["uk"],"shareid":data["share_id"].as_u64().map(|v|json!(v)).unwrap_or_else(||data["shareid"].clone())});
                return Ok(ShareListing {
                    share: share.clone(),
                    title: "百度网盘分享".into(),
                    parent_id: parent.into(),
                    files,
                    stoken: metadata.to_string(),
                });
            }
        }
        Err("百度目录超过 20000 项，请分批下载".into())
    }
    pub async fn baidu_download(
        &self,
        share: &ShareLink,
        file: &ShareFile,
        token: &str,
    ) -> Result<String, String> {
        let metadata: Value = serde_json::from_str(token).map_err(|_| "百度分享信息已过期")?;
        let account = check(
            self.json(
                self.baidu_get("/api/gettemplatevariable")
                    .query(&[("fields", r#"["bdstoken"]"#)]),
            )
            .await?,
        )?;
        let bdtoken = text(&account["result"]["bdstoken"]);
        if bdtoken.is_empty() {
            return Err("请先登录百度网盘".into());
        }
        let sign_data = check(
            self.json(self.baidu_get("/share/tplconfig").query(&[
                ("fields", "sign,timestamp"),
                ("surl", share.id.as_str()),
                ("bdstoken", bdtoken.as_str()),
            ]))
            .await?,
        )?;
        let sign = text(&sign_data["data"]["sign"]);
        let timestamp = text(&sign_data["data"]["timestamp"]);
        let uk = text(&metadata["uk"]);
        let share_id = text(&metadata["shareid"]);
        if sign.is_empty() || timestamp.is_empty() || uk.is_empty() || share_id.is_empty() {
            return Err("百度分享签名不完整，请在官方窗口打开分享后重试".into());
        }
        let cookie = self
            .transport
            .jar
            .cookies(&"https://pan.baidu.com/".parse().unwrap())
            .and_then(|v| v.to_str().ok().map(String::from))
            .unwrap_or_default();
        let encoded = cookie
            .split(';')
            .map(str::trim)
            .find_map(|v| v.strip_prefix("BDCLND="))
            .unwrap_or_default();
        let decoded = url::form_urlencoded::parse(format!("k={encoded}").as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        let body = vec![
            ("encrypt", "0".to_string()),
            ("product", "share".to_string()),
            ("uk", uk),
            ("primaryid", share_id),
            (
                "fid_list",
                json!([file.id.parse::<u64>().map_err(|_| "百度文件 ID 无效")?]).to_string(),
            ),
            ("extra", json!({"sekey":decoded}).to_string()),
        ];
        let data = check(
            self.json(
                self.transport
                    .client
                    .post("https://pan.baidu.com/api/sharedownload")
                    .header("Referer", format!("https://pan.baidu.com/s/{}", share.id))
                    .query(&[
                        ("channel", "chunlei"),
                        ("clienttype", "0"),
                        ("web", "1"),
                        ("app_id", "250528"),
                        ("sign", &sign),
                        ("timestamp", &timestamp),
                        ("bdstoken", &bdtoken),
                    ])
                    .form(&body),
            )
            .await?,
        )?;
        data["list"][0]["dlink"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .ok_or("百度未返回下载地址，请检查分享文件的下载权限".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verification_and_password_are_actionable_errors() {
        assert!(check(json!({"errno":9019})).unwrap_err().contains("验证"));
        assert!(check(json!({"errno":-9})).unwrap_err().contains("提取码"));
        assert!(check(json!({"errno":0,"list":[]})).is_ok());
    }
}
