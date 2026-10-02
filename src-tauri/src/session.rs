use crate::{model::Provider, quark::Quark};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub fn window(
    app: &tauri::AppHandle,
    provider: Provider,
    visible: bool,
) -> Result<WebviewWindow, String> {
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
        WebviewUrl::External(provider.login_url().parse().unwrap()),
    )
    .title(format!("登录{} · CloudDownloader", provider.name()))
    .inner_size(1000.0, 740.0)
    .min_inner_size(800.0, 600.0)
    .center()
    .visible(visible)
    .data_directory(path)
    .on_navigation(move |url| provider.allows_navigation(url))
    .build()
    .map_err(|e| format!("登录窗口创建失败：{e}"))?;
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
    let urls: &[&str] = match provider {
        Provider::Quark => &[
            "https://pan.quark.cn/",
            "https://drive-pc.quark.cn/",
            "https://drive-social-api.quark.cn/",
        ],
        Provider::Baidu => &["https://pan.baidu.com/", "https://passport.baidu.com/"],
    };
    for address in urls {
        let url: url::Url = address.parse().unwrap();
        for cookie in window
            .cookies_for_url(url.clone())
            .map_err(|_| "无法读取网页登录状态")?
        {
            transport.jar.add_cookie_str(&cookie.to_string(), &url);
        }
    }
    Ok(crate::providers::Session {
        provider,
        transport,
    })
}
