pub(crate) fn certificate_error(error: &reqwest::Error) -> bool {
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        if cause
            .to_string()
            .to_ascii_lowercase()
            .contains("certificate")
        {
            return true;
        }
        source = cause.source();
    }
    false
}

// Only use for read operations, including POSTs that fetch download metadata.
// HTTP responses (including authentication/quota denials) are never retried here.
pub(crate) async fn send_read_request(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, reqwest::Error> {
    for attempt in 0..2 {
        let Some(replay) = request.try_clone() else {
            break;
        };
        match replay.send().await {
            Ok(response) => return Ok(response),
            Err(error)
                if certificate_error(&error)
                    || !(error.is_request() || error.is_connect() || error.is_timeout()) =>
            {
                return Err(error)
            }
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(300 * (attempt + 1))).await
            }
        }
    }
    request.send().await
}
