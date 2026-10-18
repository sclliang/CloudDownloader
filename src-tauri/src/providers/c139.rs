use super::{
    cloud::{check, entry, Page},
    crypto, number, text, Session,
};
use crate::model::*;
use serde_json::{json, Value};

impl Session {
    #[cfg(debug_assertions)]
    pub async fn c139_diagnostics(&self) -> Result<Value, String> {
        let authenticated = !self.cloud_identity().await?.0.is_empty();
        let root = self.cloud_page("/", 1, None).await?;
        let system = root
            .entries
            .iter()
            .find(|entry| entry.file.is_dir && entry.file.id.starts_with('/'));
        let child_count = if let Some(entry) = system {
            Some(
                self.cloud_page(&entry.file.path, 1, None)
                    .await?
                    .entries
                    .len(),
            )
        } else {
            None
        };
        Ok(json!({
            "authenticated": authenticated, "rootCount": root.entries.len(),
            "systemFolderOpened": system.is_some(), "systemFolderChildCount": child_count
        }))
    }
    fn c139_authorization(&self) -> Result<String, String> {
        let value = if !self.authorization.is_empty() {
            self.authorization.clone()
        } else {
            self.cookie_value(
                &["https://yun.139.com/", "https://mail.10086.cn/"],
                "authorization",
            )
            .unwrap_or_default()
        };
        if value.is_empty() || value.len() > 16384 || value.chars().any(char::is_control) {
            return Err("请先在 139 官方窗口完成网页登录，再刷新登录状态".into());
        }
        Ok(value)
    }
    fn c139_account(&self) -> Result<String, String> {
        let auth = self.c139_authorization()?;
        let decoded = String::from_utf8(crypto::base64_decode(auth.trim_start_matches("Basic "))?)
            .map_err(|_| "139 登录状态无效，请重新登录")?;
        decoded
            .split(':')
            .nth(1)
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .map(String::from)
            .ok_or("139 登录状态缺少账号标识，请重新登录".into())
    }
    async fn c139_post(
        &self,
        host: &str,
        path: &str,
        body: Value,
        encrypted: bool,
        authenticated: bool,
    ) -> Result<Value, String> {
        let plain = body.to_string();
        let client = reqwest::Client::builder()
            .user_agent(crate::quark::USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|_| "无法初始化 139 云盘连接")?;
        let mut request = client
            .post(self.endpoint(&format!("https://{host}{path}")))
            .header("Origin", "https://yun.139.com")
            .header("Referer", "https://yun.139.com/")
            .header("Content-Type", "application/json;charset=UTF-8")
            .header("Accept", "application/json, text/plain, */*");
        if authenticated {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let random = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
            request = request
                .header("Authorization", self.c139_authorization()?)
                .header(
                    "mcloud-sign",
                    format!(
                        "{timestamp},{random},{}",
                        crypto::sign139(&plain, &timestamp, &random)
                    ),
                );
        }
        if encrypted {
            request=request.header("hcy-cool-flag","1")
                .header("x-deviceinfo","||3|12.27.0|||||chrome 150.0.0.0|360X444|zh-cn|||")
                .header("x-huawei-channelsrc","10245500").header("x-mm-source","0002")
                .header("User-Agent","Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Mobile Safari/537.36")
                .body(crypto::encrypt139(&plain)?);
        } else {
            for (key,value) in [
                ("x-yun-channel-source","10000034"),("x-yun-app-channel","10000034"),("x-huawei-channelSrc","10000034"),
                ("mcloud-version","7.17.9"),("mcloud-client","10701"),("mcloud-channel","1000101"),("mcloud-route","001"),
                ("x-yun-module-type","100"),("x-yun-api-version","v1"),("x-yun-svc-type","1"),("x-SvcType","1"),
                ("caller","web"),("x-inner-ntwk","2"),("CMS-DEVICE","default"),("x-m4c-src","10002"),("x-m4c-caller","PC"),
                ("X-Deviceinfo","||9|7.17.9|chrome|116.0.0.0|2cdaf7ada9e353c70eba99092e177991||windows 10||zh-CN|||"),
                ("x-yun-client-info","||9|7.17.9|chrome|116.0.0.0|2cdaf7ada9e353c70eba99092e177991||windows 10||zh-CN|||dW5kZWZpbmVk||"),
                ("INNER-HCY-ROUTER-HTTPS","1"),("X-Requested-With","mark.via")
            ]{request=request.header(key,value);}
            request = request.body(plain);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "连接 139 云盘失败，请检查网络或稍后重试")?;
        if !response.status().is_success() {
            return Err(format!(
                "139 接口返回 HTTP {}，请检查登录状态",
                response.status().as_u16()
            ));
        }
        let body = response.text().await.map_err(|_| "139 返回内容无效")?;
        let mut value = if encrypted {
            crypto::decrypt139(&body)?
        } else {
            serde_json::from_str(&body).map_err(|_| "139 返回内容无效")?
        };
        if encrypted
            && value.get("resultCode").is_none()
            && value.get("code").is_none()
            && value["success"].as_bool() == Some(true)
        {
            value["resultCode"] = json!("0");
        }
        check(self.provider, value)
    }
    pub async fn c139_identity(&self) -> Result<(String, String), String> {
        let account = self.c139_account()?;
        self.c139_listing("/", None).await?;
        let name = if account.len() > 7 && account.is_ascii() {
            format!("{}****{}", &account[..3], &account[account.len() - 4..])
        } else {
            "139 账号".into()
        };
        Ok((account, name))
    }
    pub async fn c139_page(
        &self,
        path: &str,
        id: &str,
        cursor: Option<&str>,
    ) -> Result<Page, String> {
        let value = self.c139_listing(id, cursor).await?;
        let rows = value["data"]["items"]
            .as_array()
            .ok_or("139 文件列表无效")?;
        let next = text(&value["data"]["nextPageCursor"]);
        Ok(Page {
            entries: rows
                .iter()
                .map(|raw| entry(self.provider, raw, path))
                .collect::<Result<_, _>>()?,
            next: (!next.is_empty()).then_some(next),
        })
    }
    async fn c139_listing(&self, id: &str, cursor: Option<&str>) -> Result<Value, String> {
        self.c139_post(
            "personal-kd-njs.yun.139.com",
            "/hcy/file/list",
            json!({
                "pageInfo": {"pageSize": 100, "pageCursor": cursor},
                "orderBy": "updated_at", "orderDirection": "DESC",
                "parentFileId": id, "imageThumbnailStyleList": ["Small", "Large"]
            }),
            false,
            true,
        )
        .await
    }
    pub async fn c139_personal_download(&self, id: &str) -> Result<String, String> {
        let value = self
            .c139_post(
                "personal-kd-njs.yun.139.com",
                "/hcy/file/getDownloadUrl",
                json!({"fileId":id}),
                false,
                true,
            )
            .await?;
        value["data"]["url"]
            .as_str()
            .map(String::from)
            .ok_or("139 未返回文件下载链接，请检查账号权限".into())
    }
    pub async fn c139_share(
        &self,
        share: &ShareLink,
        parent: &str,
    ) -> Result<ShareListing, String> {
        super::cloud::file_identifier(self.provider, parent)?;
        let mut files = Vec::new();
        for page in 0..100 {
            let value=self.c139_post("share-kd-njs.yun.139.com","/yun-share/richlifeApp/devapp/IOutLink/getOutLinkInfoV6",json!({"getOutLinkInfoReq":{"account":"","linkID":share.id,"passwd":share.passcode,"caSrt":1,"coSrt":1,"srtDr":0,"bNum":page*200+1,"eNum":(page+1)*200,"pCaID":if parent=="0"{"root"}else{parent}}}),true,false).await?;
            let data = &value["data"];
            let before = files.len();
            for (key, directory) in [("caLst", true), ("coLst", false)] {
                if let Some(rows) = data[key].as_array() {
                    for row in rows {
                        let id = text(if directory {
                            &row["caID"]
                        } else {
                            &row["coID"]
                        });
                        super::cloud::file_identifier(self.provider, &id)?;
                        files.push(ShareFile {
                            id,
                            name: text(if directory {
                                &row["caName"]
                            } else {
                                &row["coName"]
                            }),
                            size: if directory { 0 } else { number(&row["coSize"]) },
                            is_dir: directory
                                || number(&row["coType"]) == 2
                                || row["isdir"].as_bool() == Some(true),
                            parent_id: parent.into(),
                            token: String::new(),
                        });
                    }
                }
            }
            if files.len() - before < 200 {
                return Ok(ShareListing {
                    share: share.clone(),
                    title: "139 云盘分享".into(),
                    parent_id: parent.into(),
                    files,
                    stoken: String::new(),
                });
            }
        }
        Err("139 分享目录超过 20000 项，请分批下载".into())
    }
    pub async fn c139_share_download(
        &self,
        share: &ShareLink,
        file: &ShareFile,
    ) -> Result<String, String> {
        let account = self.c139_account()?;
        let value=self.c139_post("share-kd-njs.yun.139.com","/yun-share/richlifeApp/devapp/IOutLink/dlFromOutLinkV3",json!({"dlFromOutLinkReqV3":{"account":account,"linkID":share.id,"coIDLst":{"item":[file.id]},"commonAccountInfo":{"account":account,"accountType":1}}}),true,true).await?;
        value["data"]["redrUrl"]
            .as_str()
            .map(String::from)
            .ok_or("139 未返回分享下载链接，请检查账号权限".into())
    }
}
