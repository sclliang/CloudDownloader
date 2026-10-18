use crate::model::{safe_name, DirectSource, Task};
use reqwest::{header, redirect, Client, StatusCode};
use std::{path::Path, time::Duration};

pub const USER_AGENT: &str = concat!("CloudDownloader/", env!("CARGO_PKG_VERSION"));

pub struct Metadata {
    pub content_type: Option<String>,
    pub name: String,
    pub size: u64,
    pub range_supported: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub fn validate_url(input: &str) -> Result<url::Url, String> {
    let input = input.trim();
    if input.len() > 16384 || input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("请填写一个完整的 HTTP 或 HTTPS 文件链接，空格需使用 %20".into());
    }
    let mut url = url::Url::parse(input).map_err(|_| "下载链接格式无效")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("普通链接下载仅支持 HTTP 和 HTTPS".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("请使用不含账号密码的文件直链".into());
    }
    url.set_fragment(None);
    Ok(url)
}

pub fn validate_connections(connections: u8) -> Result<(), String> {
    if ![1, 4, 8, 16].contains(&connections) {
        return Err("下载连接数请选择 1、4、8 或 16".into());
    }
    Ok(())
}

fn suggested_name(url: &url::Url, headers: &header::HeaderMap) -> String {
    let disposition = headers
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    // RFC 5987 takes precedence over the ASCII fallback filename.
    for parameter in disposition.split(';').skip(1) {
        if let Some((key, value)) = parameter.trim().split_once('=') {
            if key.eq_ignore_ascii_case("filename*") {
                let parts: Vec<_> = value.trim().trim_matches('"').splitn(3, '\'').collect();
                if parts.len() == 3 && parts[0].eq_ignore_ascii_case("utf-8") {
                    if let Ok(name) = percent_encoding::percent_decode_str(parts[2]).decode_utf8() {
                        if !name.is_empty() {
                            return safe_name(&name);
                        }
                    }
                }
            }
        }
    }
    let filename =
        regex::Regex::new(r#"(?i)(?:^|;)\s*filename\s*=\s*(?:"([^"]+)"|([^;]+))"#).unwrap();
    if let Some(captures) = filename.captures(disposition) {
        if let Some(name) = captures.get(1).or_else(|| captures.get(2)) {
            return safe_name(name.as_str());
        }
    }
    let name = url
        .path_segments()
        .and_then(|mut parts| parts.next_back())
        .unwrap_or_default();
    safe_name(&percent_encoding::percent_decode_str(name).decode_utf8_lossy())
}

pub async fn probe(input: &str) -> Result<Metadata, String> {
    let url = validate_url(input)?;
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 || validate_url(attempt.url().as_str()).is_err() {
                attempt.error("Invalid download redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "无法初始化下载连接")?;
    // Read response headers only. No cloud cookies, sessions, or referer are attached.
    let response = client
        .get(url)
        .header(header::RANGE, "bytes=0-0")
        .header(header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "连接下载服务器超时，请稍后重试"
            } else {
                "无法连接下载服务器，请检查链接、网络或证书"
            }
        })?;
    let headers = response.headers();
    let content_range = headers
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let empty =
        response.status() == StatusCode::RANGE_NOT_SATISFIABLE && content_range == "bytes */0";
    if !response.status().is_success() && !empty {
        return Err(format!(
            "下载服务器返回 HTTP {}，请检查链接是否有效或需要登录",
            response.status().as_u16()
        ));
    }
    let range_supported = response.status() == StatusCode::PARTIAL_CONTENT;
    let size = if range_supported {
        content_range
            .strip_prefix("bytes 0-0/")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|size| *size > 0)
            .ok_or("服务器的分段下载响应无效")?
    } else if empty {
        0
    } else {
        headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let text_header = |key| {
        headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    Ok(Metadata {
        content_type: text_header(header::CONTENT_TYPE),
        name: suggested_name(response.url(), headers),
        size,
        range_supported,
        etag: text_header(header::ETAG).filter(|v| !v.starts_with("W/")),
        last_modified: text_header(header::LAST_MODIFIED),
    })
}

pub fn update_source(
    source: &mut DirectSource,
    old_size: u64,
    partial: bool,
    fresh: &Metadata,
) -> Result<(), String> {
    validate_url(&source.url)?;
    validate_connections(source.connections)?;
    if partial
        && ((old_size > 0 && fresh.size > 0 && old_size != fresh.size)
            || source
                .etag
                .as_ref()
                .is_some_and(|old| fresh.etag.as_ref() != Some(old))
            || (source.etag.is_none()
                && source
                    .last_modified
                    .as_ref()
                    .is_some_and(|old| fresh.last_modified.as_ref() != Some(old))))
    {
        return Err("源文件已变化，为避免文件损坏，请创建新的下载任务".into());
    }
    source.range_supported = fresh.range_supported;
    source.etag = fresh.etag.clone();
    source.last_modified = fresh.last_modified.clone();
    Ok(())
}

pub fn available_name(directory: &Path, name: &str, existing: &[Task]) -> String {
    let base = safe_name(name);
    let mut candidate = base.clone();
    let mut number = 1;
    while directory.join(&candidate).exists()
        || directory.join(format!("{candidate}.aria2")).exists()
        || existing.iter().any(|t| {
            t.directory
                .eq_ignore_ascii_case(&directory.to_string_lossy())
                && t.name.eq_ignore_ascii_case(&candidate)
        })
    {
        let file = Path::new(&base);
        candidate = format!(
            "{} ({number}){}",
            file.file_stem().unwrap_or_default().to_string_lossy(),
            file.extension()
                .map(|v| format!(".{}", v.to_string_lossy()))
                .unwrap_or_default()
        );
        number += 1;
    }
    candidate
}

pub fn restart_without_ranges(directory: &Path, name: &str) -> Result<(), String> {
    if safe_name(name) != name {
        return Err("下载文件名无效".into());
    }
    let file = directory.join(name);
    let checkpoint = directory.join(format!("{name}.aria2"));
    if !checkpoint.is_file() {
        if file.exists() {
            return Err("同名文件存在且没有续传记录，请新建下载任务".into());
        }
        return Ok(());
    }
    // Only discard this task's incomplete output, identified by its aria2 checkpoint.
    // Non-range servers cannot resume; starting clean also avoids aria2's fallback loop.
    if file.exists() {
        std::fs::remove_file(file)
            .map_err(|_| "无法重新下载，请检查未完成文件是否被其他程序占用")?;
    }
    std::fs::remove_file(checkpoint).map_err(|_| "无法重置下载进度，请检查目录权限")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn direct_urls_and_filenames_have_safe_boundaries() {
        assert_eq!(
            validate_url(" https://example.test/a%20b.zip?key=value#part ")
                .unwrap()
                .as_str(),
            "https://example.test/a%20b.zip?key=value"
        );
        assert!(validate_url("http://127.0.0.1:9876/local.bin").is_ok());
        for url in [
            "file:///C:/test",
            "ftp://example.test/a",
            "javascript:alert(1)",
            "https://user:password@example.test/a",
            "https://example.test/a\r\nHeader:x",
            "https://a.test/a https://b.test/b",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
        for n in [0, 2, 3, 17, 255] {
            assert!(validate_connections(n).is_err());
        }
        let url = url::Url::parse("https://example.test/%E4%B8%AD%E6%96%87.zip?key=value").unwrap();
        let mut headers = header::HeaderMap::new();
        assert_eq!(suggested_name(&url, &headers), "中文.zip");
        headers.insert(
            header::CONTENT_DISPOSITION,
            header::HeaderValue::from_static(
                "attachment; filename=download.zip; filename*=UTF-8''%E9%B8%A3%E6%BD%AE.zip",
            ),
        );
        assert_eq!(suggested_name(&url, &headers), "鸣潮.zip");
        headers.insert(
            header::CONTENT_DISPOSITION,
            header::HeaderValue::from_static("attachment; filename=\"../../CON.txt\""),
        );
        assert!(!suggested_name(&url, &headers).contains(['/', '\\']));
    }

    #[test]
    fn refuses_changed_source_before_resuming_and_preserves_file_collisions() {
        let source = DirectSource {
            url: "https://example.test/a".into(),
            connections: 8,
            range_supported: true,
            etag: Some("\"v1\"".into()),
            last_modified: None,
        };
        let mut fresh = Metadata {
            content_type: None,
            name: "a.bin".into(),
            size: 100,
            range_supported: true,
            etag: Some("\"v1\"".into()),
            last_modified: None,
        };
        assert!(update_source(&mut source.clone(), 100, true, &fresh).is_ok());
        fresh.size = 101;
        assert!(update_source(&mut source.clone(), 100, true, &fresh).is_err());
        fresh.size = 100;
        fresh.etag = Some("\"v2\"".into());
        assert!(update_source(&mut source.clone(), 100, true, &fresh).is_err());
        fresh.etag = None;
        assert!(update_source(&mut source.clone(), 100, true, &fresh).is_err());
        assert!(update_source(&mut source.clone(), 100, false, &fresh).is_ok());
        let directory =
            std::env::temp_dir().join(format!("clouddownloader-name-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("file.zip"), b"existing").unwrap();
        std::fs::write(directory.join("file (1).zip.aria2"), b"checkpoint").unwrap();
        assert_eq!(available_name(&directory, "file.zip", &[]), "file (2).zip");
        assert_eq!(
            std::fs::read(directory.join("file.zip")).unwrap(),
            b"existing"
        );
        std::fs::remove_file(directory.join("file.zip")).unwrap();
        std::fs::remove_file(directory.join("file (1).zip.aria2")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn probes_redirects_ranges_plain_files_and_errors_without_cloud_credentials() {
        let fixture = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = fixture.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = fixture.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 4096];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let Ok(n) = socket.read(&mut buffer).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..n]);
                    }
                    let request = String::from_utf8_lossy(&request).to_lowercase();
                    let response = if request.contains("cookie:")
                        || request.contains("authorization:")
                    {
                        "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"
                    } else if request.starts_with("get /redirect ") {
                        "HTTP/1.1 302 Found\r\nLocation: /file\r\nContent-Length: 0\r\n\r\n"
                    } else if request.starts_with("get /file ") {
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/99\r\nContent-Length: 1\r\nContent-Disposition: attachment; filename*=UTF-8''%E9%B8%A3%E6%BD%AE.zip\r\nETag: \"v1\"\r\n\r\na"
                    } else if request.starts_with("get /plain ") {
                        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nfile"
                    } else if request.starts_with("get /empty ") {
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */0\r\nContent-Length: 0\r\n\r\n"
                    } else {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"
                    };
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        let ranged = probe(&format!("http://{address}/redirect")).await.unwrap();
        assert_eq!(ranged.name, "鸣潮.zip");
        assert_eq!(ranged.size, 99);
        assert!(ranged.range_supported);
        assert_eq!(ranged.etag.as_deref(), Some("\"v1\""));
        let plain = probe(&format!("http://{address}/plain")).await.unwrap();
        assert_eq!(plain.size, 4);
        assert!(!plain.range_supported);
        assert_eq!(
            probe(&format!("http://{address}/empty"))
                .await
                .unwrap()
                .size,
            0
        );
        assert!(
            probe(&format!("http://{address}/missing?secret=not-for-errors"))
                .await
                .err()
                .unwrap()
                .contains("HTTP 404")
        );
        server.abort();
    }
}
