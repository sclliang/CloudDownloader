// UC wire protocol reviewed against CYQawa/YunX, revision recorded in notices.
use super::{
    cloud::{check, entry, Page},
    number, text, Download, Session,
};
use crate::model::*;
use serde_json::{json, Value};
const API: &str = "https://pc-api.uc.cn/1/clouddrive/";
const ORIGIN: &str = "https://drive.uc.cn";
const GUEST_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) uc-cloud-drive/2.5.20 Chrome/100.0.4896.160 Electron/18.3.5.4-b478491100 Safari/537.36 Channel/pckk_other_ch";
const CLOUD_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) uc-cloud-drive/1.6.1 Chrome/100.0.4896.160 Electron/18.3.5.16-b62cf9c50d Safari/537.36 Channel/ucpan_other_ch";

impl Session {
    async fn uc_call(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value, String> {
        let origin = if path == "transfer_share/detail" {
            "https://fast.uc.cn"
        } else {
            ORIGIN
        };
        let mut request = self
            .transport
            .client
            .request(method, self.endpoint(&format!("{API}{path}")))
            .query(&[("pr", "UCBrowser"), ("fr", "pc")])
            .query(query)
            .header("Origin", origin)
            .header("Referer", format!("{origin}/"));
        if path == "transfer_share/detail" {
            request = request.query(&[("entry", "ft")]);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        check(Provider::Uc, self.json(request).await?)
    }
    pub(super) async fn uc_identity(&self) -> Result<(String, String), String> {
        let value = self
            .json(
                self.transport
                    .client
                    .get(self.endpoint(&format!("{ORIGIN}/account/info")))
                    .query(&[("fr", "pc"), ("platform", "pc")])
                    .header("Referer", format!("{ORIGIN}/")),
            )
            .await?;
        if value["success"].as_bool() != Some(true) {
            return Err("请先在官方窗口登录 UC 网盘".into());
        }
        let data = &value["data"];
        let id = ["uid", "user_id", "userId", "ukey"]
            .iter()
            .map(|k| text(&data[*k]))
            .find(|v| !v.is_empty())
            .or_else(|| self.cookie_value(&[ORIGIN], "__uid"))
            .ok_or("未取得 UC 账号标识，请重新登录")?;
        let name = ["nickname", "nick_name", "nickName"]
            .iter()
            .map(|k| text(&data[*k]))
            .find(|v| !v.is_empty())
            .unwrap_or_else(|| "UC 账号".into());
        Ok((id, name))
    }
    pub(super) async fn uc_page(
        &self,
        path: &str,
        parent: &str,
        page: u32,
    ) -> Result<Page, String> {
        let value = self
            .uc_call(
                reqwest::Method::GET,
                "file/sort",
                &[
                    ("pdir_fid", parent.into()),
                    ("_page", page.to_string()),
                    ("_size", "100".into()),
                    ("_fetch_total", "1".into()),
                    ("_sort", "file_type:asc,updated_at:desc".into()),
                ],
                None,
            )
            .await?;
        let rows = value["data"]["list"].as_array().ok_or("UC 文件列表无效")?;
        let entries = rows
            .iter()
            .map(|raw| entry(Provider::Uc, raw, path))
            .collect::<Result<Vec<_>, _>>()?;
        let total = number(&value["metadata"]["_total"]);
        Ok(Page {
            entries,
            next: (rows.len() == 100 && (total == 0 || total > page as u64 * 100))
                .then(|| page.to_string()),
        })
    }
    pub(super) async fn uc_share(
        &self,
        share: &ShareLink,
        parent: &str,
    ) -> Result<ShareListing, String> {
        super::cloud::identifier(&share.id)?;
        super::cloud::identifier(parent)?;
        let value = self
            .uc_call(
                reqwest::Method::POST,
                "share/sharepage/token",
                &[],
                Some(
                    json!({"pwd_id":share.id,"passcode":share.passcode,"share_for_transfer":true}),
                ),
            )
            .await?;
        let token = text(&value["data"]["stoken"]);
        if token.is_empty() {
            return Err("未取得 UC 分享令牌，请检查提取码".into());
        }
        let title = text(&value["data"]["title"]);
        let mut files = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for page in 1..=200 {
            let value = self
                .uc_call(
                    reqwest::Method::GET,
                    "transfer_share/detail",
                    &[
                        ("pwd_id", share.id.clone()),
                        ("pdir_fid", parent.into()),
                        ("stoken", token.clone()),
                        ("passcode", share.passcode.clone()),
                        ("fetch_file_list", "1".into()),
                        ("_page", page.to_string()),
                        ("_size", "100".into()),
                        ("_fetch_total", "1".into()),
                        ("_fetch_share", "1".into()),
                    ],
                    None,
                )
                .await?;
            let rows = value["data"]["list"]
                .as_array()
                .or_else(|| value["data"]["detail_info"]["list"].as_array())
                .ok_or("UC 分享列表无效")?;
            for raw in rows {
                let item = entry(Provider::Uc, raw, "/")?.file;
                if !seen.insert(item.id.clone()) {
                    return Err("UC 分享分页重复，请重新解析".into());
                }
                files.push(ShareFile {
                    id: item.id,
                    name: item.name,
                    size: item.size,
                    is_dir: item.is_dir,
                    parent_id: parent.into(),
                    token: text(&raw["share_fid_token"]),
                });
            }
            if rows.len() < 100 {
                return Ok(ShareListing {
                    share: share.clone(),
                    title: if title.is_empty() {
                        "UC 分享".into()
                    } else {
                        title
                    },
                    parent_id: parent.into(),
                    stoken: token,
                    files,
                });
            }
        }
        Err("UC 分享超过分页上限，请选择子目录".into())
    }
    pub(super) async fn uc_download(
        &self,
        id: &str,
        share: Option<(&ShareLink, &ShareFile, &str)>,
    ) -> Result<Download, String> {
        super::cloud::identifier(id)?;
        let mut body = json!({"fids":[id]});
        if let Some((share, file, token)) = share {
            body["pwd_id"] = json!(share.id);
            body["stoken"] = json!(token);
            body["fids_token"] = json!([file.token]);
        }
        let guest = share.is_some()
            && self
                .cookie_value_raw(&["https://pc-api.uc.cn/", ORIGIN], "__pus")
                .is_none()
            && self
                .cookie_value_raw(&["https://pc-api.uc.cn/", ORIGIN], "__puus")
                .is_none();
        let agent = if share.is_none() {
            CLOUD_AGENT
        } else if guest {
            GUEST_AGENT
        } else {
            crate::quark::USER_AGENT
        };
        let mut request = self
            .transport
            .client
            .post(self.endpoint(&format!("{API}file/download")))
            .query(&[("pr", "UCBrowser"), ("fr", "pc")])
            .header("Origin", ORIGIN)
            .header("Referer", format!("{ORIGIN}/"))
            .header("User-Agent", agent)
            .json(&body);
        request = if share.is_some() {
            request.query(&[("entry", "ft")])
        } else {
            request.query(&[("sys", "win32"), ("ve", "1.6.1")])
        };
        if guest {
            request = request.header(
                "Sec-CH-UA",
                r#""Not=A?Brand";v="99", "Chromium";v="100", "Google Chrome";v="100""#,
            );
        }
        let value = check(Provider::Uc, self.json(request).await?)?;
        let url = text(&value["data"][0]["download_url"]);
        let mut download = self.download_descriptor(url)?;
        download.user_agent = agent.into();
        // UC's CDN callback needs the API-issued download session. Only these
        // three provider-specific cookies may be sent to validated UC CDN hosts.
        let cookie = ["__pugs", "__puus", "__pus"]
            .iter()
            .filter_map(|key| {
                self.cookie_value_raw(&["https://pc-api.uc.cn/", ORIGIN], key)
                    .map(|v| format!("{key}={v}"))
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !cookie.is_empty() {
            download.headers.push(format!("Cookie: {cookie}"));
        }
        Ok(download)
    }
}
