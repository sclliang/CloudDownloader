use crate::{model::Provider, pan115_session, quark::Quark};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

// A registered window can be observed before its CookieManager restoration
// finishes. All 115 callers must wait for the complete initialization sequence.
static PAN115_INITIALIZATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
static ILANZOU_DEVICE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn window(
    app: &tauri::AppHandle,
    provider: Provider,
    visible: bool,
) -> Result<WebviewWindow, String> {
    let _initializing = if provider == Provider::Pan115 {
        Some(
            PAN115_INITIALIZATION
                .lock()
                .map_err(|_| "115 登录窗口初始化失败，请重启程序")?,
        )
    } else {
        None
    };
    let label = format!("{}-login", provider.key());
    if let Some(window) = app.get_webview_window(&label) {
        if visible {
            window.show().map_err(|e| e.to_string())?;
            window.unminimize().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
        }
        return Ok(window);
    }
    let path = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(format!("{}-profile", provider.key()));
    let window = WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::External(
            if provider == Provider::Pan115 {
                "about:blank"
            } else {
                provider.login_url()
            }
            .parse()
            .unwrap(),
        ),
    )
    .title(format!("登录{} · CloudDownloader", provider.name()))
    .inner_size(1000.0, 740.0)
    .min_inner_size(800.0, 600.0)
    .center()
    .visible(visible)
    .data_directory(path)
    .on_navigation(move |url| {
        (provider == Provider::Pan115 && url.as_str() == "about:blank")
            || provider.allows_navigation(url)
    })
    .build()
    .map_err(|e| format!("登录窗口创建失败：{e}"))?;
    if provider == Provider::Pan115 {
        let path = cache_path(app)?;
        let restored = pan115_session::load(&path)
            .and_then(|cookies| pan115_session::restore(&window, cookies));
        if let Some(state) = app.try_state::<crate::AppState>() {
            *state.session_cache_error.lock().unwrap() = restored.err().unwrap_or_default();
        }
        // Cookie writes and navigation are queued to the same WebView thread.
        window
            .navigate(provider.login_url().parse().unwrap())
            .map_err(|_| "无法打开 115 登录页面")?;
    }
    let close = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = close.hide();
        }
    });
    Ok(window)
}

pub async fn connect(
    app: &tauri::AppHandle,
    provider: Provider,
) -> Result<crate::providers::Session, String> {
    let window = window(app, provider, false)?;
    let transport = Quark::new()?;
    let mut browser_cookies = Vec::new();
    let urls: &[&str] = match provider {
        Provider::Quark => &[
            "https://pan.quark.cn/",
            "https://drive-pc.quark.cn/",
            "https://drive-social-api.quark.cn/",
        ],
        Provider::Baidu => &["https://pan.baidu.com/", "https://passport.baidu.com/"],
        Provider::Pan115 => &[
            "https://115.com/",
            "https://webapi.115.com/",
            "https://proapi.115.com/",
        ],
        Provider::Pan123 => &[
            "https://www.123pan.com/",
            "https://yun.123pan.cn/",
            "https://www.123865.com/",
        ],
        Provider::C139 => &[
            "https://yun.139.com/",
            "https://mail.10086.cn/",
            "https://personal-kd-njs.yun.139.com/",
        ],
        Provider::Uc => &[
            "https://drive.uc.cn/",
            "https://pc-api.uc.cn/",
            "https://fast.uc.cn/",
        ],
        Provider::Lanzou => &["https://pc.woozooo.com/", "https://accounts.woozooo.com/"],
        Provider::Ilanzou => &[
            "https://www.ilanzou.com/",
            "https://apis.ilanzou.com/",
            "https://apix.ilanzou.com/",
        ],
    };
    for address in urls {
        let url: url::Url = address.parse().unwrap();
        for cookie in window
            .cookies_for_url(url.clone())
            .map_err(|_| "无法读取网页登录状态")?
        {
            transport.jar.add_cookie_str(&cookie.to_string(), &url);
            if provider == Provider::Pan115 {
                browser_cookies.push(cookie);
            }
        }
    }
    if provider == Provider::Pan115 {
        let _ = persist_pan115(app, browser_cookies);
    }
    let device_id = if provider == Provider::Pan123 {
        let state = app.state::<crate::AppState>();
        let store = state.store.lock().unwrap();
        match store
            .setting("pan123_device_id")
            .filter(|value| uuid::Uuid::parse_str(value).is_ok())
        {
            Some(value) => value,
            None => {
                let value = uuid::Uuid::new_v4().to_string();
                store.set_setting("pan123_device_id", &value)?;
                value
            }
        }
    } else {
        String::new()
    };
    let mut session = crate::providers::Session {
        provider,
        transport,
        device_id,
        #[cfg(test)]
        test_endpoint: None,
        authorization: if matches!(provider, Provider::Pan123 | Provider::C139) {
            read_authorization(&window, provider)
                .await
                .unwrap_or_default()
        } else {
            String::new()
        },
    };
    if provider == Provider::Ilanzou {
        let _device_guard = ILANZOU_DEVICE.lock().await;
        session.authorization = session
            .cookie_value(
                &["https://www.ilanzou.com/", "https://ilanzou.com/"],
                "appToken",
            )
            .unwrap_or_default();
        let saved = app
            .state::<crate::AppState>()
            .store
            .lock()
            .unwrap()
            .setting("ilanzou_device_id");
        session.device_id = match saved.filter(|id| {
            (8..=128).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        }) {
            Some(id) => id,
            None => {
                let id = session.ilanzou_uuid().await?;
                app.state::<crate::AppState>()
                    .store
                    .lock()
                    .unwrap()
                    .set_setting("ilanzou_device_id", &id)?;
                id
            }
        };
    }
    Ok(session)
}

fn cache_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|root| root.join("pan115-session.dat"))
        .map_err(|_| "无法定位 115 登录资料目录".into())
}

fn persist_pan115(
    app: &tauri::AppHandle,
    cookies: Vec<tauri::webview::cookie::Cookie<'static>>,
) -> Result<(), String> {
    let result = cache_path(app).and_then(|path| pan115_session::save(&path, cookies));
    if let Some(state) = app.try_state::<crate::AppState>() {
        *state.session_cache_error.lock().unwrap() =
            result.as_ref().err().cloned().unwrap_or_default();
    }
    result
}

// Run on a worker while the event loop is still alive: CookieManager callbacks
// require the UI thread, so reading cookies from an Exit event would deadlock.
pub fn save_before_exit(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("pan115-login") {
        let cookies = window
            .cookies()
            .map_err(|_| "无法读取退出前的 115 登录资料")?;
        persist_pan115(app, cookies)?;
    }
    Ok(())
}

#[cfg(windows)]
async fn read_authorization(window: &WebviewWindow, provider: Provider) -> Option<String> {
    // Execute only in this provider's isolated login WebView. Remote pages receive
    // no new IPC permissions, and credentials never pass through the main UI.
    let url = window.url().ok()?;
    let hosts: &[&str] = match provider {
        Provider::Pan123 => &["123pan.com", "123pan.cn", "123865.com", "123912.com"],
        Provider::C139 => &["139.com", "10086.cn"],
        _ => return None,
    };
    if url.scheme() != "https"
        || !hosts
            .iter()
            .any(|host| crate::model::host_matches(url.host_str().unwrap_or_default(), host))
    {
        return None;
    }
    let keys = match provider {
        Provider::Pan123 => r#"["authorToken","token","access_token"]"#,
        _ => r#"["authorization","Authorization"]"#,
    };
    let script = format!(
        r#"(() => {{ for (const store of [localStorage, sessionStorage]) for (const key of {keys}) {{ let value = store.getItem(key); if (!value) continue; try {{ const parsed = JSON.parse(value); value = typeof parsed === 'string' ? parsed : parsed.token || parsed.authorToken || parsed.authorization || ''; }} catch {{}} if (typeof value === 'string' && value.length > 8 && value.length <= 16384 && !/[\x00-\x1f\x7f]/.test(value)) return value; }} return ''; }})()"#
    );
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            if let Ok(core) = unsafe { webview.controller().CoreWebView2() } {
                let handler = webview2_com::ExecuteScriptCompletedHandler::create(Box::new(
                    move |result, value| {
                        let _ = send.send(if result.is_ok() {
                            serde_json::from_str::<String>(&value).ok()
                        } else {
                            None
                        });
                        Ok(())
                    },
                ));
                let script = webview2_com::CoTaskMemPWSTR::from(script.as_str());
                let _ = unsafe { core.ExecuteScript(*script.as_ref().as_pcwstr(), &handler) };
            }
        })
        .ok()?;
    tokio::time::timeout(std::time::Duration::from_secs(4), receive)
        .await
        .ok()?
        .ok()?
        .filter(|value| !value.is_empty())
}

#[cfg(not(windows))]
async fn read_authorization(_: &WebviewWindow, _: Provider) -> Option<String> {
    None
}
