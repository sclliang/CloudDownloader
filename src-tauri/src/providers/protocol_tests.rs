use super::{cloud, crypto, pan123, *};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/protocol-vectors.json")).unwrap()
}

#[test]
fn protocol_vectors_from_python_crc32_md5_aes_and_rsa() {
    let v = vectors();
    for name in ["pan115_short", "pan115_long"] {
        let key = std::array::from_fn(|i| i as u8);
        assert_eq!(
            crypto::encode115(v[name]["payload"].as_str().unwrap(), &key).unwrap(),
            v[name]["base64"]
        );
    }
    let s = &v["pan123"];
    let (key, value) = crypto::sign123(
        s["path"].as_str().unwrap(),
        s["seconds"].as_i64().unwrap(),
        s["random"].as_u64().unwrap() as u32,
    );
    assert_eq!(key, s["key"]);
    assert_eq!(value, s["value"]);
    let s = &v["c139_signature"];
    assert_eq!(
        crypto::sign139(
            s["body"].as_str().unwrap(),
            s["timestamp"].as_str().unwrap(),
            s["random"].as_str().unwrap()
        ),
        s["signature"]
    );
    let expected: Value =
        serde_json::from_str(v["c139_response"]["plain"].as_str().unwrap()).unwrap();
    for field in ["plain", "ciphertext", "gzip"] {
        assert_eq!(
            crypto::decrypt139(v["c139_response"][field].as_str().unwrap()).unwrap(),
            expected
        );
    }
    let encrypted = crypto::encrypt139(&expected.to_string()).unwrap();
    assert_eq!(crypto::decrypt139(&encrypted).unwrap(), expected);
}

#[test]
fn malformed_protocol_responses_fail_without_panics() {
    for value in ["", "!not-base64!", "AA==", "[]", "null"] {
        assert!(crypto::decode115(value, &[0; 16]).is_err());
        assert!(crypto::decrypt139(value).is_err());
    }
    assert!(crypto::decode115(&STANDARD.encode([255; 128]), &[0; 16]).is_err());
    let mut ciphertext = STANDARD
        .decode(vectors()["c139_response"]["ciphertext"].as_str().unwrap())
        .unwrap();
    ciphertext.truncate(ciphertext.len() - 1);
    assert!(crypto::decrypt139(&STANDARD.encode(ciphertext)).is_err());
}

#[test]
fn pan115_expired_session_has_a_specific_login_message() {
    let message =
        cloud::check(Provider::Pan115, json!({"state":false,"errno":990001})).unwrap_err();
    assert!(message.contains("990001") && message.contains("重新登录"));
    assert!(!message.contains("会员") && !message.contains("提取码"));
}

#[test]
fn new_share_routes_and_code_precedence() {
    for (address, provider, id, code) in [
        (
            "https://115cdn.com/s/swfixture?password=AB12",
            Provider::Pan115,
            "swfixture",
            "AB12",
        ),
        (
            "https://115rc.com/s/swfixture",
            Provider::Pan115,
            "swfixture",
            "",
        ),
        (
            "https://115.com/swfixture-Ab12/",
            Provider::Pan115,
            "swfixture",
            "Ab12",
        ),
        (
            "https://www.123865.com/s/Fixture-123",
            Provider::Pan123,
            "Fixture-123",
            "",
        ),
        (
            "https://42.share.123pan.cn/123pan/Fixture-123?pwd=AB12",
            Provider::Pan123,
            "Fixture-123",
            "AB12",
        ),
        (
            "https://www.123pan.cn/api/srr?sk=Fixture-123&st=s",
            Provider::Pan123,
            "Fixture-123",
            "",
        ),
        (
            "https://yun.139.com/shareweb/#/w/i/fixture123?pwd=AB12",
            Provider::C139,
            "fixture123",
            "AB12",
        ),
    ] {
        let share = parse_share(address, "").unwrap();
        assert_eq!(
            (share.provider, share.id.as_str(), share.passcode.as_str()),
            (provider, id, code),
            "{address}"
        );
        assert_eq!(parse_share(address, "Cd34").unwrap().passcode, "Cd34");
        assert_eq!(serde_json::to_value(provider).unwrap(), provider.key());
    }
    assert_eq!(
        parse_share("https://115.com/s/swfixture 访问码：AB12", "")
            .unwrap()
            .passcode,
        "AB12"
    );
    for invalid in [
        "https://115cdn.com.evil.test/s/swfixture",
        "https://evil.123pan.cn/s/Fixture-123",
        "https://share.123pan.cn.evil.test/123pan/Fixture-123",
        "https://yun.139.com/#/w/i/fixture",
        "https://yun.139.com/shareweb/#/w/i/../secret",
        "https://www.123pan.cn/api/srr?sk=..%2Fsecret",
        "https://115.com:444/s/swfixture",
        "https://yun.139.com@evil.test/shareweb/#/w/i/fixture",
    ] {
        assert!(parse_share(invalid, "").is_err(), "{invalid}");
    }
}

#[test]
fn source_metadata_fixtures_use_opaque_ids_and_do_not_serialize_secrets() {
    for (provider, raw, is_dir, size) in [
        (
            Provider::Quark,
            json!({"fid":"q-file","file_name":"夸克.txt","size":32,"dir":false,"updated_at":1700000000000u64,"md5":"checksum","download_url":"secret-url"}),
            false,
            32,
        ),
        (
            Provider::Pan115,
            json!({"cid":"115-dir","n":"资料","s":"0"}),
            true,
            0,
        ),
        (
            Provider::Pan115,
            json!({"fid":"115-file","cid":"115-dir","n":"文件.txt","s":"128","pc":"secret-pickcode"}),
            false,
            128,
        ),
        (
            Provider::Pan123,
            json!({"FileId":123,"FileName":"123.txt","Size":64,"Type":0,"Etag":"checksum","S3KeyFlag":"secret-object"}),
            false,
            64,
        ),
        (
            Provider::C139,
            json!({"fileId":"139-folder","name":"资料","type":"folder","size":0}),
            true,
            0,
        ),
    ] {
        let item = cloud::entry(provider, &raw, "/parent").unwrap();
        assert_eq!(item.file.is_dir, is_dir);
        assert_eq!(item.file.size, size);
        assert_eq!(item.file.path, format!("/parent/{}", item.file.id));
        assert!(!serde_json::to_string(&item.file)
            .unwrap()
            .contains("secret"));
    }
    for invalid in ["", ".", "..", "a/b", "a\\b", "a\r\nb", "%2F"] {
        assert!(cloud::identifier(invalid).is_err());
    }
    assert_eq!(cloud::parent_id("/", Provider::C139).unwrap(), "/");
    assert_eq!(cloud::parent_id("/", Provider::Pan123).unwrap(), "0");
    assert!(cloud::parent_id("/../file", Provider::Quark).is_err());
    let error = cloud::check(
        Provider::Pan123,
        json!({"code":401,"message":"secret-token"}),
    )
    .unwrap_err();
    assert!(!error.contains("secret"));
}

#[test]
fn download_wrappers_and_provider_credential_boundaries() {
    let target = "https://cdn.123pancloud.com/file?token=fixture";
    let encoded = STANDARD.encode(target);
    for value in [
        target.to_string(),
        encoded.clone(),
        format!("https://www.123pan.com/download-v2?params={encoded}"),
    ] {
        assert_eq!(pan123::decode_download(&value).unwrap(), target);
    }
    for (provider, address) in [
        (Provider::Pan115, "https://cdn.115cdn.com/file"),
        (Provider::Pan123, target),
        (Provider::C139, "https://obs.myhuaweicloud.com/file"),
    ] {
        let transport = Quark::new().unwrap();
        transport.jar.add_cookie_str(
            "authorization=fixture-secret; Path=/",
            &address.parse().unwrap(),
        );
        let session = Session {
            provider,
            transport,
            authorization: "fixture-bearer".into(),
            device_id: String::new(),
            test_endpoint: None,
        };
        let download = session.download_descriptor(address.into()).unwrap();
        assert!(download
            .headers
            .iter()
            .all(|header| !header.to_lowercase().starts_with("cookie:")
                && !header.to_lowercase().starts_with("authorization:")));
        for bad in [
            "https://evil.test/file",
            "https://127.0.0.1/file",
            "https://cdn.115cdn.com.evil.test/file",
            "https://yun.139.com@evil.test/file",
            "http://cdn.123pancloud.com/file",
        ] {
            assert!(session.download_descriptor(bad.into()).is_err());
        }
    }
}

struct Request {
    target: String,
    headers: std::collections::HashMap<String, String>,
    body: String,
}
async fn fixture(
    provider: Provider,
    responses: Vec<Value>,
) -> (Session, tokio::task::JoinHandle<Vec<Request>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut input = Vec::new();
            let request = loop {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0 && input.len() < 65536);
                input.extend_from_slice(&buf[..n]);
                if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8(input[..end].to_vec()).unwrap();
                    let headers: std::collections::HashMap<_, _> = head
                        .lines()
                        .skip(1)
                        .filter_map(|line| line.split_once(':'))
                        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let length = headers
                        .get("content-length")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    if input.len() >= end + 4 + length {
                        break Request {
                            target: head
                                .lines()
                                .next()
                                .unwrap()
                                .split_whitespace()
                                .nth(1)
                                .unwrap()
                                .into(),
                            headers,
                            body: String::from_utf8(input[end + 4..end + 4 + length].to_vec())
                                .unwrap(),
                        };
                    }
                }
            };
            requests.push(request);
            let body = response.to_string();
            let wire = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(wire.as_bytes()).await.unwrap();
        }
        requests
    });
    let transport = Quark::new().unwrap();
    transport
        .jar
        .add_cookie_str("fixture_session=secret; Path=/", &endpoint.parse().unwrap());
    transport.jar.add_cookie_str(
        "UID=115fixture_A1; Path=/",
        &"https://115.com/".parse().unwrap(),
    );
    let authorization = if provider == Provider::C139 {
        format!("Basic {}", STANDARD.encode("pc:13800138000:fixture"))
    } else {
        "fixture-bearer".into()
    };
    (
        Session {
            provider,
            transport,
            authorization,
            device_id: "00000000-0000-4000-8000-000000000001".into(),
            test_endpoint: Some(endpoint),
        },
        server,
    )
}

#[tokio::test]
async fn quark_personal_download_refreshes_metadata_before_issuing_url() {
    let raw =
        json!({"fid":"q123","file_name":"fixture.txt","size":32,"dir":false,"md5":"checksum"});
    let (session, server) = fixture(
        Provider::Quark,
        vec![
            json!({"success":true,"data":{"uid":"quark-fixture"}}),
            json!({"code":0,"data":{"list":[raw.clone()]},"metadata":{"_total":1}}),
            json!({"code":0,"data":[{"download_url":"https://cdn.quark.cn/fixture"}]}),
        ],
    )
    .await;
    let source = OwnSource {
        account_id: crypto::md5_hex("quark:quark-fixture"),
        file: cloud::entry(Provider::Quark, &raw, "/").unwrap().file,
    };
    let download = session.cloud_download(&source).await.unwrap();
    assert_eq!(download.url, "https://cdn.quark.cn/fixture");
    let requests = server.await.unwrap();
    assert!(requests[1].target.contains("pdir_fid=0"));
    assert_eq!(
        serde_json::from_str::<Value>(&requests[2].body).unwrap()["fids"],
        json!(["q123"])
    );
    assert!(requests[2].headers["user-agent"].contains("QuarkPC/"));
}

#[tokio::test]
async fn pan123_personal_download_uses_bearer_signature_and_stable_device_id() {
    let raw = json!({"FileId":123,"FileName":"fixture.txt","Size":32,"Type":0,"Etag":"checksum","S3KeyFlag":"object"});
    let (session, server) = fixture(
        Provider::Pan123,
        vec![
            json!({"code":0,"data":{"UID":1234,"Nickname":"fixture"}}),
            json!({"code":0,"data":{"InfoList":[raw.clone()],"Next":"-1"}}),
            json!({"code":0,"data":{"DownloadUrl":"https://cdn.123pancloud.com/fixture"}}),
        ],
    )
    .await;
    let source = OwnSource {
        account_id: crypto::md5_hex("pan123:1234"),
        file: cloud::entry(Provider::Pan123, &raw, "/").unwrap().file,
    };
    let download = session.cloud_download(&source).await.unwrap();
    assert_eq!(download.headers.len(), 1);
    let requests = server.await.unwrap();
    for request in &requests {
        assert_eq!(request.headers["authorization"], "Bearer fixture-bearer");
        assert_eq!(request.headers["loginuuid"], session.device_id);
        assert!(
            request.headers.contains_key("auth-key") && request.headers.contains_key("auth-value")
        );
    }
    let payload: Value = serde_json::from_str(&requests[2].body).unwrap();
    assert_eq!(payload["fileId"], 123);
    assert_eq!(payload["s3keyFlag"], "object");
}

#[tokio::test]
async fn c139_personal_download_uses_authorization_without_browser_cookies() {
    let raw = json!({"fileId":"139-file","name":"fixture.txt","size":32,"type":"file"});
    let listing =
        json!({"code":"0000","success":true,"data":{"items":[raw.clone()],"nextPageCursor":""}});
    let (session, server) = fixture(Provider::C139, vec![listing.clone(), listing, json!({"code":"0000","success":true,"data":{"url":"https://obs.myhuaweicloud.com/fixture"}})]).await;
    let source = OwnSource {
        account_id: crypto::md5_hex("c139:13800138000"),
        file: cloud::entry(Provider::C139, &raw, "/").unwrap().file,
    };
    let download = session.cloud_download(&source).await.unwrap();
    assert_eq!(download.headers.len(), 1);
    let requests = server.await.unwrap();
    for request in &requests {
        assert_eq!(request.headers["authorization"], session.authorization);
        assert!(!request.headers.contains_key("cookie"));
        assert!(request.headers.contains_key("mcloud-sign"));
    }
    assert_eq!(
        serde_json::from_str::<Value>(&requests[0].body).unwrap()["parentFileId"],
        "/"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&requests[2].body).unwrap()["fileId"],
        "139-file"
    );
}

#[tokio::test]
async fn switched_account_stops_before_requesting_file_or_download_url() {
    let (session, server) = fixture(
        Provider::Pan123,
        vec![json!({"code":0,"data":{"UID":5678}})],
    )
    .await;
    let source = OwnSource {
        account_id: crypto::md5_hex("pan123:1234"),
        file: cloud::entry(
            Provider::Pan123,
            &json!({"FileId":123,"FileName":"fixture.txt","Type":0}),
            "/",
        )
        .unwrap()
        .file,
    };
    assert!(session
        .cloud_download(&source)
        .await
        .err()
        .unwrap()
        .contains("账号已切换"));
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn pan115_identity_and_share_normalize_file_and_folder_entries() {
    let (session, server) = fixture(Provider::Pan115, vec![json!({"state":true,"data":"fixture user"}), json!({"state":true,"data":{"list":[{"cid":"folder","n":"资料","fc":"0"},{"fid":"file","n":"fixture.txt","s":"32","fc":"1"}],"count":2}})]).await;
    assert_eq!(
        session.cloud_identity().await.unwrap().0,
        crypto::md5_hex("pan115:115fixture")
    );
    let listing = session
        .list(
            &ShareLink {
                provider: Provider::Pan115,
                id: "swfixture".into(),
                passcode: "AB12".into(),
            },
            "0",
        )
        .await
        .unwrap();
    assert!(listing.files[0].is_dir);
    assert_eq!(listing.files[1].size, 32);
    let requests = server.await.unwrap();
    assert!(
        requests[1].target.contains("share_code=swfixture")
            && requests[1].target.contains("receive_code=AB12")
    );
}

#[tokio::test]
async fn pan123_share_honors_empty_next_cursor_and_omits_empty_password() {
    let row = json!({"FileId":123,"FileName":"fixture.txt","Size":32,"Type":0,"Etag":"checksum","S3KeyFlag":"object"});
    let (session, server) = fixture(
        Provider::Pan123,
        vec![
            json!({"code":0,"data":{"InfoList":[row],"Next":""}}),
            json!({"code":0,"data":{"InfoList":[],"Next":"-1"}}),
        ],
    )
    .await;
    let listing = session
        .list(
            &ShareLink {
                provider: Provider::Pan123,
                id: "Fixture-123".into(),
                passcode: String::new(),
            },
            "0",
        )
        .await
        .unwrap();
    assert_eq!(listing.files.len(), 1);
    assert!(!listing.files[0].token.is_empty());
    assert!(!serde_json::to_string(&listing)
        .unwrap()
        .contains("S3KeyFlag"));
    let requests = server.await.unwrap();
    assert!(!requests[0].target.contains("SharePwd"));
    assert!(requests[1].target.contains("next=&") && requests[1].target.contains("Page=2"));
}

#[tokio::test]
async fn c139_share_listing_is_encrypted_and_anonymous() {
    let (session, server) = fixture(Provider::C139, vec![json!({"resultCode":"0","success":true,"data":{"caLst":[{"caID":"folder","caName":"资料"}],"coLst":[{"coID":"file","coName":"fixture.txt","coSize":"32","coType":1}]}})]).await;
    let listing = session
        .list(
            &ShareLink {
                provider: Provider::C139,
                id: "share139".into(),
                passcode: "AB12".into(),
            },
            "0",
        )
        .await
        .unwrap();
    assert_eq!(listing.files.len(), 2);
    let requests = server.await.unwrap();
    let request = &requests[0];
    assert_eq!(request.headers["hcy-cool-flag"], "1");
    assert!(
        !request.headers.contains_key("authorization")
            && !request.headers.contains_key("mcloud-sign")
            && !request.headers.contains_key("cookie")
    );
    let body = crypto::decrypt139(&request.body).unwrap();
    assert_eq!(body["getOutLinkInfoReq"]["passwd"], "AB12");
    assert_eq!(body["getOutLinkInfoReq"]["pCaID"], "root");
}

#[test]
fn c139_system_folder_ids_are_encoded_as_one_path_component() {
    let raw = json!({"fileId":"/app-folder","name":"应用收藏","type":"folder","size":0});
    let entry = cloud::entry(Provider::C139, &raw, "/").unwrap();
    assert_eq!(entry.file.id, "/app-folder");
    assert_eq!(entry.file.path, "/%2Fapp-folder");
    assert_eq!(
        cloud::parent_id(&entry.file.path, Provider::C139).unwrap(),
        "/app-folder"
    );
    assert_eq!(
        cloud::parent_id("/%2fapp-folder", Provider::C139).unwrap(),
        "/app-folder"
    );
    let child = cloud::entry(
        Provider::C139,
        &json!({"fileId":"child","name":"file.txt","type":"file"}),
        &entry.file.path,
    )
    .unwrap();
    assert_eq!(child.file.path, "/%2Fapp-folder/child");
    assert_eq!(
        cloud::parent_id(&child.file.path, Provider::C139).unwrap(),
        "child"
    );
    for provider in [Provider::Quark, Provider::Pan115, Provider::Pan123] {
        assert!(cloud::file_identifier(provider, "/app-folder").is_err());
        assert!(cloud::parent_id("/%2Fapp-folder", provider).is_err());
    }
    for bad in [
        "/",
        "//folder",
        "/..",
        "/../folder",
        "/folder/child",
        "/bad\\name",
        "/bad\nname",
    ] {
        assert!(cloud::file_identifier(Provider::C139, bad).is_err());
    }
    for bad in [
        "/%2F..",
        "/%2F%2Ffolder",
        "/%2Ffolder%2Fchild",
        "/%252Ffolder",
    ] {
        assert!(cloud::parent_id(bad, Provider::C139).is_err());
    }
}

#[test]
fn pan123_opens_personal_cloud_and_accepts_the_official_sso_redirect() {
    assert_eq!(Provider::Pan123.login_url(), "https://yun.123pan.cn/");
    for address in [Provider::Pan123.login_url(), "https://user.123pan.cn/centerlogin?redirect_url=https%3A%2F%2Fyun.123pan.cn%2F%3Fnotoken%3D1&source_page=website"] {
        assert!(Provider::Pan123.allows_navigation(&address.parse().unwrap()));
    }
    for address in [
        "https://user.123pan.cn.evil.test/centerlogin",
        "http://user.123pan.cn/centerlogin",
        "https://evil.test/?redirect_url=https://yun.123pan.cn/",
    ] {
        assert!(!Provider::Pan123.allows_navigation(&address.parse().unwrap()));
    }
}

#[tokio::test]
async fn c139_system_folder_browsing_and_download_preserve_original_api_ids() {
    let root = json!({"code":"0000","success":true,"data":{"items":[{"fileId":"/app-folder","name":"应用收藏","type":"folder","size":0}],"nextPageCursor":""}});
    let children = json!({"code":"0000","success":true,"data":{"items":[{"fileId":"/fixture-file","name":"fixture.txt","type":"file","size":32}],"nextPageCursor":""}});
    let (session, server) = fixture(Provider::C139, vec![root.clone(), root.clone(), children.clone(), root, children, json!({"code":"0000","success":true,"data":{"url":"https://obs.myhuaweicloud.com/fixture"}})]).await;
    let account_id = session.cloud_identity().await.unwrap().0;
    let root = session.cloud_page("/", 1, None).await.unwrap();
    let folder = &root.entries[0].file;
    assert_eq!(folder.path, "/%2Fapp-folder");
    let children = session.cloud_page(&folder.path, 1, None).await.unwrap();
    let source = OwnSource {
        account_id,
        file: children.entries.into_iter().next().unwrap().file,
    };
    assert_eq!(source.file.path, "/%2Fapp-folder/%2Ffixture-file");
    assert!(session.cloud_download(&source).await.is_ok());
    let requests = server.await.unwrap();
    for index in [2, 4] {
        assert_eq!(
            serde_json::from_str::<Value>(&requests[index].body).unwrap()["parentFileId"],
            "/app-folder"
        );
    }
    assert_eq!(
        serde_json::from_str::<Value>(&requests[5].body).unwrap()["fileId"],
        "/fixture-file"
    );
}
