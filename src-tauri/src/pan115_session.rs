use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::Path, sync::Mutex};
use tauri::webview::cookie::{time::OffsetDateTime, Cookie, SameSite};

pub const COOKIE_NAMES: [&str; 7] = [
    "UID",
    "CID",
    "SEID",
    "KID",
    "PHPSESSID",
    "GST",
    "USERSESSIONID",
];
#[cfg(windows)]
pub fn restore(window: &tauri::WebviewWindow, cookies: Vec<Cookie<'static>>) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_2, COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
        COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE, COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
    };
    use windows::core::{Interface, HSTRING};
    if cookies.is_empty() {
        return Ok(());
    }
    let (send, receive) = std::sync::mpsc::channel();
    window
        .with_webview(move |view| {
            let result: windows::core::Result<()> = (|| unsafe {
                let manager = view
                    .controller()
                    .CoreWebView2()?
                    .cast::<ICoreWebView2_2>()?
                    .CookieManager()?;
                for cookie in cookies {
                    // cookie::Cookie strips the leading domain dot. Restore the 115
                    // login domain cookie through COM so its API subdomains see it.
                    let domain = match cookie.domain() {
                        Some("115.com") => ".115.com",
                        Some(domain) => domain,
                        None => ".115.com",
                    };
                    let native = manager.CreateCookie(
                        &HSTRING::from(cookie.name()),
                        &HSTRING::from(cookie.value()),
                        &HSTRING::from(domain),
                        &HSTRING::from(cookie.path().unwrap_or("/")),
                    )?;
                    native.SetExpires(
                        cookie
                            .expires_datetime()
                            .map(|time| time.unix_timestamp() as f64)
                            .unwrap_or(-1.0),
                    )?;
                    native.SetIsSecure(cookie.secure().unwrap_or(true))?;
                    native.SetIsHttpOnly(cookie.http_only().unwrap_or(false))?;
                    native.SetSameSite(match cookie.same_site() {
                        Some(SameSite::Strict) => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
                        Some(SameSite::None) => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
                        _ => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
                    })?;
                    manager.AddOrUpdateCookie(&native)?;
                }
                Ok(())
            })();
            let _ = send.send(result.is_ok());
        })
        .map_err(|_| "无法恢复 115 登录资料")?;
    if receive
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("无法恢复 115 登录资料，请重新登录".into())
    }
}
#[cfg(not(windows))]
pub fn restore(_: &tauri::WebviewWindow, _: Vec<Cookie<'static>>) -> Result<(), String> {
    Err("当前系统不支持 Windows 登录资料恢复".into())
}
const MAX_BYTES: u64 = 256 * 1024;
static FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct SavedCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: Option<String>,
    expires: Option<i64>,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u8,
    cookies: Vec<SavedCookie>,
}

impl SavedCookie {
    fn valid(&self) -> bool {
        COOKIE_NAMES.contains(&self.name.as_str())
            && !self.value.is_empty()
            && self.value.len() <= 16384
            && !self.value.chars().any(|c| c.is_control() || c == ';')
            && crate::model::host_matches(self.domain.trim_start_matches('.'), "115.com")
            && self.path.starts_with('/')
            && self.path.len() <= 2048
            && !self.path.chars().any(char::is_control)
            && self
                .expires
                .is_none_or(|at| at > crate::model::now() as i64)
            && self
                .same_site
                .as_ref()
                .is_none_or(|value| ["Lax", "Strict", "None"].contains(&value.as_str()))
    }
    fn into_cookie(self) -> Cookie<'static> {
        let mut cookie = Cookie::new(self.name, self.value);
        cookie.set_domain(self.domain);
        cookie.set_path(self.path);
        cookie.set_secure(self.secure);
        cookie.set_http_only(self.http_only);
        if let Some(at) = self
            .expires
            .and_then(|at| OffsetDateTime::from_unix_timestamp(at).ok())
        {
            cookie.set_expires(at);
        }
        if let Some(site) = self.same_site {
            cookie.set_same_site(match site.as_str() {
                "Strict" => SameSite::Strict,
                "None" => SameSite::None,
                _ => SameSite::Lax,
            });
        }
        cookie
    }
}

pub fn load(path: &Path) -> Result<Vec<Cookie<'static>>, String> {
    let _lock = FILE_LOCK.lock().unwrap();
    if !path.exists() {
        return Ok(Vec::new());
    }
    if !path.is_file() || path.metadata().map_err(|_| "无法读取 115 登录资料")?.len() > MAX_BYTES
    {
        return Err("115 登录资料格式无效，请重新登录".into());
    }
    let encrypted = std::fs::read(path).map_err(|_| "无法读取 115 登录资料")?;
    let plain = crypt(&encrypted, false)?;
    let saved: Snapshot =
        serde_json::from_slice(&plain).map_err(|_| "115 登录资料格式无效，请重新登录")?;
    if saved.version != 1 || saved.cookies.len() > 32 {
        return Err("115 登录资料版本无效".into());
    }
    let cookies: Vec<_> = saved
        .cookies
        .into_iter()
        .filter(SavedCookie::valid)
        .collect();
    if !["UID", "SEID"]
        .iter()
        .all(|name| cookies.iter().any(|cookie| cookie.name == *name))
    {
        return Ok(Vec::new());
    }
    Ok(cookies.into_iter().map(SavedCookie::into_cookie).collect())
}

pub fn save(path: &Path, cookies: Vec<Cookie<'static>>) -> Result<(), String> {
    let _lock = FILE_LOCK.lock().unwrap();
    let mut selected = BTreeMap::new();
    for cookie in cookies {
        let saved = SavedCookie {
            name: cookie.name().into(),
            value: cookie.value().into(),
            domain: cookie.domain().unwrap_or("115.com").to_ascii_lowercase(),
            path: cookie.path().unwrap_or("/").into(),
            secure: cookie.secure().unwrap_or(true),
            http_only: cookie.http_only().unwrap_or(false),
            same_site: cookie.same_site().map(|site| site.to_string()),
            expires: cookie.expires_datetime().map(|at| at.unix_timestamp()),
        };
        if saved.valid() {
            selected.insert(
                (saved.name.clone(), saved.domain.clone(), saved.path.clone()),
                saved,
            );
        }
    }
    if !["UID", "SEID"]
        .iter()
        .all(|name| selected.values().any(|cookie| cookie.name == *name))
    {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("无法清除已退出的 115 登录资料".into()),
        }
    }
    if selected.len() > 32 {
        return Err("115 登录资料条目过多".into());
    }
    let snapshot = Snapshot {
        version: 1,
        cookies: selected.into_values().collect(),
    };
    let plain = serde_json::to_vec(&snapshot).map_err(|_| "无法保存 115 登录资料")?;
    let encrypted = crypt(&plain, true)?;
    if encrypted.len() as u64 > MAX_BYTES {
        return Err("115 登录资料过大".into());
    }
    let parent = path.parent().ok_or("115 登录资料目录无效")?;
    std::fs::create_dir_all(parent).map_err(|_| "无法创建 115 登录资料目录")?;
    let temporary = parent.join(format!(".pan115-session-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "无法保存 115 登录资料，请检查本地资料目录权限")?;
        file.write_all(&encrypted)
            .and_then(|_| file.sync_all())
            .map_err(|_| "无法保存 115 登录资料，请检查磁盘空间")?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|_| "无法更新 115 登录资料")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn crypt(data: &[u8], protect: bool) -> Result<Vec<u8>, String> {
    use windows::{
        core::w,
        Win32::{
            Foundation::{LocalFree, HLOCAL},
            Security::Cryptography::{
                CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
            },
        },
    };
    if data.is_empty() || data.len() as u64 > MAX_BYTES {
        return Err("115 登录资料格式无效".into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr().cast_mut(),
    };
    let entropy_data = b"sixunited.CloudDownloader.pan115.session.v1";
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy_data.len() as u32,
        pbData: entropy_data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        if protect {
            CryptProtectData(
                &input,
                w!("CloudDownloader 115 session"),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    result.map_err(|_| "无法使用当前 Windows 用户加密或读取 115 登录资料")?;
    if output.pbData.is_null() {
        return Err("115 登录资料加密结果无效".into());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
    }
    Ok(bytes)
}
#[cfg(not(windows))]
fn crypt(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("当前系统不支持 Windows 登录资料加密".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    fn cookies() -> Vec<Cookie<'static>> {
        ["UID", "SEID"]
            .into_iter()
            .map(|name| {
                Cookie::build((name, format!("synthetic-{name}-secret")))
                    .domain(".115.com")
                    .path("/")
                    .secure(true)
                    .http_only(true)
                    .same_site(SameSite::Lax)
                    .build()
            })
            .collect()
    }
    #[test]
    fn encrypted_session_cookies_survive_reopen_and_atomic_update() {
        let folder = std::env::temp_dir().join(format!("cloud-cookie-{}", uuid::Uuid::new_v4()));
        let path = folder.join("session.dat");
        save(&path, cookies()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes
            .windows(b"synthetic".len())
            .any(|part| part == b"synthetic"));
        let restored = load(&path).unwrap();
        assert_eq!(restored.len(), 2);
        assert!(restored
            .iter()
            .all(|cookie| cookie.expires_datetime().is_none()
                && cookie.secure() == Some(true)
                && cookie.http_only() == Some(true)
                && cookie.same_site() == Some(SameSite::Lax)));
        let mut refreshed = cookies();
        refreshed[0].set_value("synthetic-new");
        save(&path, refreshed).unwrap();
        assert!(load(&path)
            .unwrap()
            .iter()
            .any(|cookie| cookie.value() == "synthetic-new"));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn rejects_tampering_foreign_domains_expiration_and_clears_logout() {
        let folder = std::env::temp_dir().join(format!("cloud-cookie-{}", uuid::Uuid::new_v4()));
        let path = folder.join("session.dat");
        save(&path, cookies()).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 128;
        std::fs::write(&path, bytes).unwrap();
        assert!(load(&path).is_err());
        let mut candidates = cookies();
        candidates.push(
            Cookie::build(("CID", "foreign-secret"))
                .domain("115.com.evil.test")
                .path("/")
                .build(),
        );
        candidates.push(
            Cookie::build(("KID", "expired-secret"))
                .domain(".115.com")
                .path("/")
                .expires(OffsetDateTime::UNIX_EPOCH)
                .build(),
        );
        save(&path, candidates).unwrap();
        assert_eq!(load(&path).unwrap().len(), 2);
        save(&path, Vec::new()).unwrap();
        assert!(!path.exists());
        assert!(load(&path).unwrap().is_empty());
        std::fs::remove_dir(folder).unwrap();
    }
}
