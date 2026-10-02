use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Quark,
    Baidu,
}

impl Provider {
    pub const ALL: [Self; 2] = [Self::Quark, Self::Baidu];
    pub fn key(self) -> &'static str {
        match self {
            Self::Quark => "quark",
            Self::Baidu => "baidu",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Quark => "夸克网盘",
            Self::Baidu => "百度网盘",
        }
    }
    pub fn origin(self) -> &'static str {
        match self {
            Self::Quark => "https://pan.quark.cn",
            Self::Baidu => "https://pan.baidu.com",
        }
    }
    pub fn login_url(self) -> &'static str {
        match self {
            Self::Quark => "https://pan.quark.cn/list#/list/all",
            Self::Baidu => "https://pan.baidu.com/disk/main#/index?category=all",
        }
    }
    pub fn allows_navigation(self, url: &url::Url) -> bool {
        let domains: &[&str] = match self {
            Self::Quark => &["quark.cn", "uc.cn", "quark.com", "alipay.com"],
            Self::Baidu => &["baidu.com"],
        };
        url.scheme() == "https"
            && domains
                .iter()
                .any(|d| host_matches(url.host_str().unwrap_or_default(), d))
    }
}

pub fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareLink {
    // v0.1 task records contain Quark shares without a provider field.
    #[serde(default)]
    pub provider: Provider,
    pub id: String,
    pub passcode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub parent_id: String,
    #[serde(skip)]
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareListing {
    #[serde(skip)]
    pub stoken: String,
    pub share: ShareLink,
    pub title: String,
    pub parent_id: String,
    pub files: Vec<ShareFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub provider: Provider,
    pub logged_in: bool,
    pub name: String,
    pub message: String,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            provider: Provider::Quark,
            logged_in: false,
            name: "未登录".into(),
            message: "登录账号后开始下载".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub completed: u64,
    pub speed: u64,
    pub status: String,
    pub error: String,
    pub directory: String,
    pub created_at: u64,
    pub gid: Option<String>,
    pub share: ShareLink,
    pub file: ShareFile,
    #[serde(default)]
    pub own: Option<OwnSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified: u64,
    pub md5: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnSource {
    pub account_id: String,
    pub file: DriveFile,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveListing {
    pub account_id: String,
    pub path: String,
    pub page: u32,
    pub has_more: bool,
    pub files: Vec<DriveFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedShare {
    pub id: String,
    pub url: String,
    pub password: String,
    pub period: u32,
    pub created_at: u64,
    pub account_id: String,
    pub names: Vec<String>,
    #[serde(default)]
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub accounts: Vec<Account>,
    pub tasks: Vec<Task>,
    pub download_dir: String,
    pub engine_running: bool,
    pub engine_error: String,
    pub uploads: Vec<crate::upload::UploadView>,
    pub shares: Vec<CreatedShare>,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut cleaned = cleaned.trim().trim_end_matches([' ', '.']).to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        cleaned = "download".into();
    }
    let stem = cleaned
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| ('1'..='9').contains(&c)))
    {
        cleaned.insert(0, '_');
    }
    // Keep enough room for the directory and aria2 control-file suffix.
    if cleaned.chars().count() > 180 {
        cleaned = cleaned.chars().take(180).collect();
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_names_cannot_escape_directory_or_use_devices() {
        assert_eq!(safe_name("../../secret"), ".._.._secret");
        assert_eq!(safe_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_name("LPT9.mp4"), "_LPT9.mp4");
        assert_eq!(safe_name("中文片名.mp4"), "中文片名.mp4");
        assert_eq!(safe_name(".."), "download");
    }
}
