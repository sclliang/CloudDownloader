use crate::model::{ShareFile, ShareLink, ShareListing};
#[cfg(test)]
use crate::providers::parse_share;
use reqwest::{cookie::Jar, Client, Method};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use url::Url;

pub const ORIGIN: &str = "https://pan.quark.cn";
pub const API: &str = "https://drive-pc.quark.cn/1/clouddrive/";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
pub const DOWNLOAD_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 QuarkPC/6.9.7.761 QuarkCloudDrivePC/6.9.7.761 quark-cloud-drive/2.5.40";

#[derive(Clone)]
pub struct Quark {
    pub client: Client,
    pub jar: Arc<Jar>,
}

impl Quark {
    pub fn new() -> Result<Self, String> {
        let jar = Arc::new(Jar::default());
        let client = Client::builder()
            .cookie_provider(jar.clone())
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(12))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, jar })
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value, String> {
        let mut url = Url::parse(&format!("{API}{path}")).map_err(|_| "接口地址无效")?;
        url.query_pairs_mut()
            .append_pair("pr", "ucpro")
            .append_pair("fr", "pc");
        for (key, value) in query {
            url.query_pairs_mut().append_pair(key, value);
        }
        let mut req = self
            .client
            .request(method, url)
            .header("Referer", format!("{ORIGIN}/"))
            .header("Origin", ORIGIN);
        if path == "file/download" {
            req = req
                .header("User-Agent", DOWNLOAD_AGENT)
                .query(&[("sys", "win32"), ("ve", "6.9.7.761")]);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                "连接夸克超时，请稍后重试".to_string()
            } else {
                "连接夸克失败，请检查网络".to_string()
            }
        })?;
        let status = response.status();
        let payload: Value = response.json().await.map_err(|_| {
            format!(
                "夸克接口 {path} 返回 HTTP {}，请在登录窗口检查验证提示",
                status.as_u16()
            )
        })?;
        if !status.is_success() || payload.get("code").and_then(Value::as_i64) != Some(0) {
            let code = payload["code"].as_i64().unwrap_or(-1);
            let msg = payload["message"].as_str().unwrap_or("请求未成功");
            return Err(format!(
                "夸克：{msg}（接口 {path}，HTTP {}，错误码 {code}）",
                status.as_u16()
            ));
        }
        Ok(payload)
    }

    pub async fn account(&self) -> Result<String, String> {
        let response = self
            .client
            .get(format!("{ORIGIN}/account/info"))
            .query(&[("fr", "pc"), ("platform", "pc")])
            .header("Referer", format!("{ORIGIN}/"))
            .send()
            .await
            .map_err(|_| "无法检查登录状态，请检查网络")?;
        let payload: Value = response.json().await.map_err(|_| "登录状态响应无效")?;
        account_name(&payload)
    }

    async fn token(&self, share: &ShareLink) -> Result<(String, String), String> {
        let data = self.call(Method::POST, "share/sharepage/token", &[], Some(json!({"pwd_id":share.id,"passcode":share.passcode,"support_visit_limit_private_share":true}))).await?;
        let token = data["data"]["stoken"]
            .as_str()
            .ok_or("未取得分享令牌，请检查提取码")?;
        let title = data["data"]["title"].as_str().unwrap_or("夸克分享");
        Ok((token.into(), title.into()))
    }

    pub async fn list(&self, share: &ShareLink, parent: &str) -> Result<ShareListing, String> {
        let (token, title) = self.token(share).await?;
        let mut files = Vec::new();
        for page in 1..=200 {
            let result = self
                .call(
                    Method::GET,
                    "share/sharepage/detail",
                    &[
                        ("pwd_id", share.id.clone()),
                        ("stoken", token.clone()),
                        ("pdir_fid", parent.into()),
                        ("_page", page.to_string()),
                        ("_size", "100".into()),
                        ("_fetch_total", "1".into()),
                        ("_fetch_share", "1".into()),
                    ],
                    None,
                )
                .await?;
            let batch = result["data"]["list"]
                .as_array()
                .ok_or("分享文件列表无效")?;
            for item in batch {
                files.push(ShareFile {
                    id: item["fid"].as_str().ok_or("文件 ID 缺失")?.into(),
                    name: item["file_name"].as_str().unwrap_or("未命名文件").into(),
                    size: item["size"].as_u64().unwrap_or(0),
                    is_dir: item["file"].as_bool() == Some(false)
                        || item["dir"].as_bool() == Some(true),
                    parent_id: parent.into(),
                    token: item["share_fid_token"].as_str().unwrap_or_default().into(),
                });
            }
            if batch.len() < 100 {
                return Ok(ShareListing {
                    stoken: token,
                    share: share.clone(),
                    title,
                    parent_id: parent.into(),
                    files,
                });
            }
        }
        Err("该目录文件过多，请分目录选择下载".into())
    }

    pub async fn download(
        &self,
        share: &ShareLink,
        file: &ShareFile,
        token: &str,
    ) -> Result<String, String> {
        let grant = self.download_grant().await?;
        let result = self.call(Method::POST, "file/download", &[], Some(json!({"fids":[file.id],"fids_token":[file.token],"pwd_id":share.id,"stoken":token,"token":grant,"speedup_session":""}))).await?;
        let url = result["data"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v["download_url"].as_str())
            .ok_or("未取得下载地址，请检查账号权限或在夸克中确认该分享仍有效")?;
        validate_download_url(url)?;
        Ok(url.into())
    }

    async fn download_grant(&self) -> Result<String, String> {
        let stamp = crate::model::now();
        let response = self.client.post("https://drive-social-api.quark.cn/1/clouddrive/chat/conv/file/acquire_dl_token")
            .query(&[("pr", "ucpro"), ("fr", "pc"), ("sys", "win32"), ("ve", "6.9.7.761"), ("la", "zh-CN"), ("ch", "pckk@product_guanwan")])
            .header("User-Agent", DOWNLOAD_AGENT)
            .header("Referer", format!("{ORIGIN}/"))
            .json(&json!({"conversation_id":format!("300000{stamp}"),"conversation_type":3,"msg_id":format!("{stamp}000")}))
            .send().await.map_err(|_| "申请下载凭据失败，请检查网络")?;
        let status = response.status();
        let payload: Value = response
            .json()
            .await
            .map_err(|_| "下载凭据响应无效，请在登录窗口检查验证提示")?;
        if !status.is_success() || payload["code"].as_i64() != Some(0) {
            return Err(format!(
                "夸克下载凭据：{}（HTTP {}，错误码 {}）",
                payload["message"].as_str().unwrap_or("申请失败"),
                status.as_u16(),
                payload["code"]
            ));
        }
        payload["data"]["token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(String::from)
            .ok_or("夸克没有返回下载凭据".into())
    }
}

fn account_name(payload: &Value) -> Result<String, String> {
    if payload["success"].as_bool() == Some(true)
        && payload["data"]
            .as_object()
            .is_some_and(|data| !data.is_empty())
    {
        return Ok(["nickname", "nick_name", "nickName", "name"]
            .iter()
            .filter_map(|key| payload["data"][*key].as_str())
            .find(|name| !name.trim().is_empty())
            .unwrap_or("夸克账号")
            .into());
    }
    Err("请在夸克登录窗口完成登录".into())
}

fn validate_download_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|_| "夸克返回的下载地址无效")?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || ![
            "quark.cn",
            "quark.com",
            "uc.cn",
            "ucweb.com",
            "aliyuncs.com",
        ]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return Err("下载地址不属于已知夸克下载域名".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anonymous_success_response_is_not_a_logged_in_account() {
        assert!(account_name(&json!({"success":true,"code":"OK","data":{}})).is_err());
        assert!(account_name(&json!({"success":false,"data":{"nickname":"Someone"}})).is_err());
        assert_eq!(
            account_name(&json!({"success":true,"data":{"nickname":"测试账号"}})).unwrap(),
            "测试账号"
        );
    }
    #[test]
    fn parses_share_text_and_code_without_accepting_lookalike_hosts() {
        let s = parse_share(
            "我分享了文件\n链接：https://pan.quark.cn/s/74108d40cd8e\n提取码：UsAv",
            "",
        )
        .unwrap();
        assert_eq!(s.id, "74108d40cd8e");
        assert_eq!(s.passcode, "UsAv");
        assert!(parse_share("https://pan.quark.cn.evil.test/s/abc", "").is_err());
        assert_eq!(
            parse_share("https://pan.quark.cn/s/abc?pwd=Ab12", "")
                .unwrap()
                .passcode,
            "Ab12"
        );
    }
    #[test]
    fn rejects_download_urls_that_would_leak_cookies() {
        assert!(validate_download_url("https://dl-vdrive.quark.cn/a").is_ok());
        assert!(validate_download_url("https://quark.cn.evil.test/a").is_err());
        assert!(validate_download_url("http://localhost/a").is_err());
    }
}
