use std::sync::OnceLock;

pub fn message(code: &str, detail: &str, direct: bool) -> String {
    // aria2 details can contain signed URLs. Extract only the numeric HTTP status;
    // never persist or display the raw engine response.
    static HTTP_STATUS: OnceLock<regex::Regex> = OnceLock::new();
    let status = HTTP_STATUS
        .get_or_init(|| regex::Regex::new(r"\bstatus=(\d{3})\b").unwrap())
        .captures(detail)
        .and_then(|c| c[1].parse::<u16>().ok());
    if let Some(status) = status {
        let description = match status {
            401 | 403 if direct => "服务器拒绝访问，请检查链接权限或获取新的文件直链",
            401 | 403 => {
                "网盘服务器拒绝访问，重试会重新获取地址；若仍失败，请检查账号下载权限或完成网页验证"
            }
            404 | 410 if direct => "文件不存在或链接已失效，请获取有效直链后新建任务",
            404 | 410 => "文件不存在或链接已失效，点击重试将重新获取地址",
            416 => "服务器无法提供所需分段，请刷新来源后重试",
            429 => "服务器请求过于频繁，请稍后重试",
            500..=599 => "下载服务器暂时异常，请稍后重试",
            _ => "服务器返回了异常响应，可点击重试",
        };
        return format!("{description}（HTTP {status}）");
    }
    match code {
        "3" if direct => "文件不存在或链接不可用，请获取有效直链后新建任务".into(),
        "3" => "文件不存在或链接不可用，点击重试将重新获取地址".into(),
        "8" => "服务器无法续传此文件，请刷新来源后重新下载".into(),
        "9" => "磁盘空间不足，请释放空间后重试".into(),
        "13" => "文件已存在，请检查下载目录".into(),
        "22" => "下载服务器返回了异常响应，可点击重试".into(),
        _ => {
            let code = code
                .parse::<u32>()
                .map(|v| v.to_string())
                .unwrap_or_else(|_| "未知".into());
            format!("下载中断（错误 {code}），可点击重试继续")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_errors_are_specific_and_never_expose_engine_details() {
        let error = message("22", "URI=https://d.pcs.baidu.com/file?sign=secret\nThe response status is not successful. status=403", false);
        assert!(error.contains("HTTP 403") && error.contains("拒绝访问"));
        assert!(
            !error.contains("失效") && !error.contains("secret") && !error.contains("https://")
        );
        for (status, expected) in [
            (404, "不存在"),
            (410, "失效"),
            (416, "分段"),
            (429, "频繁"),
            (503, "暂时异常"),
        ] {
            assert!(message("22", &format!("status={status}"), false).contains(expected));
        }
        assert!(message("22", "status=403", true).contains("直链"));
        assert!(message("9", "private", false).contains("磁盘空间"));
        assert!(message("22", "private", false).contains("异常响应"));
        assert!(!message("secret", "private", false).contains("secret"));
    }
}
