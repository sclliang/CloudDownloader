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
            "https://www.123295.com/s/Fixture-123?pwd=Ab12",
            Provider::Pan123,
            "Fixture-123",
            "Ab12",
        ),
        (
            "https://123684.com/s/Fixture-123",
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
        "https://www.123295.com.example.org/s/Fixture-123",
        "https://cdn.123295.com/s/Fixture-123",
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
        format!("https://www.123pan.com/download?params={encoded}"),
        STANDARD.encode(format!(
            "https://www.123pan.com/download-v2?params={encoded}"
        )),
    ] {
        assert_eq!(
            pan123::decode_download(&value, None).unwrap().as_str(),
            target
        );
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

#[test]
fn pan123_download_nodes_require_the_documented_host_and_port_pairs() {
    for address in [
        "https://user-other-free-download-cdn.123295.com:443/file",
        "https://another-node.123295.com/file",
        "https://download-cdn.cjjd19.com/file",
        "https://node.pd1.cjjd19.com/file",
        "https://node.pd1.cjjd19.com:30443/file",
        "https://node.d.cjjd15.com:30443/file",
    ] {
        assert!(validate_download(Provider::Pan123, address).is_ok());
        assert!(validate_download(Provider::Quark, address).is_err());
    }
    for (address, reason, node) in [
        (
            "https://cdn.123295.com.example.org/private?sig=hidden",
            "下载域名",
            "cdn.123295.com.example.org:443",
        ),
        (
            "https://cdn.123295.com:30443/private?sig=hidden",
            "下载端口",
            "cdn.123295.com:30443",
        ),
        (
            "https://node.pd1.cjjd19.com.example.org/private?sig=hidden",
            "下载域名",
            "node.pd1.cjjd19.com.example.org:443",
        ),
        (
            "https://yun.123pan.cn:30443/private?sig=hidden",
            "下载端口",
            "yun.123pan.cn:30443",
        ),
        (
            "https://node.d.cjjd15.com:9443/private?sig=hidden",
            "下载端口",
            "node.d.cjjd15.com:9443",
        ),
        (
            "https://user:password@node.pd1.cjjd19.com/private?sig=hidden",
            "用户信息",
            "node.pd1.cjjd19.com:443",
        ),
    ] {
        let error = validate_download(Provider::Pan123, address).unwrap_err();
        assert!(error.contains(reason) && error.contains(node));
        for redacted in ["private", "sig", "hidden", "user", "password"] {
            assert!(!error.contains(redacted));
        }
    }
}

struct Request {
    target: String,
    headers: std::collections::HashMap<String, String>,
    body: String,
}

#[test]
fn eight_provider_routes_preserve_lanzou_domains_and_passwords() {
    for (address, provider, id, password) in [
        (
            "https://drive.uc.cn/s/FixtureUC?pwd=Ab12",
            Provider::Uc,
            "FixtureUC",
            "Ab12",
        ),
        (
            "https://fast.uc.cn/s/FixtureUC",
            Provider::Uc,
            "FixtureUC",
            "",
        ),
        (
            "https://www.ilanzou.com/s/AbC123",
            Provider::Ilanzou,
            "AbC123",
            "",
        ),
        (
            "http://someone.lanzoub.com/bFixture?pwd=123",
            Provider::Lanzou,
            "https://someone.lanzoub.com/bFixture",
            "123",
        ),
        (
            "https://pan.lanzoui.com/iFixture 密码：一二三",
            Provider::Lanzou,
            "https://pan.lanzoui.com/iFixture",
            "一二三",
        ),
    ] {
        let share = parse_share(address, "").unwrap();
        assert_eq!(
            (share.provider, share.id.as_str(), share.passcode.as_str()),
            (provider, id, password)
        );
    }
    for bad in [
        "https://lanzou.com.evil.test/iFile",
        "https://evililanzou.com/s/file",
        "https://drive.uc.cn@evil.test/s/file",
        "https://fake.uc.cn/s/file",
        "https://lanzoub.com:444/iFile",
    ] {
        assert!(parse_share(bad, "").is_err(), "{bad}");
    }
    assert_eq!(Provider::ALL.len(), 8);
    for provider in Provider::ALL {
        assert_eq!(serde_json::to_value(provider).unwrap(), provider.key());
    }
    assert!(Provider::Uc.allows_navigation(&"https://account.uc.cn/".parse().unwrap()));
    assert!(Provider::Lanzou.allows_navigation(&"https://accounts.woozooo.com/".parse().unwrap()));
    assert!(!Provider::Lanzou
        .allows_navigation(&"https://accounts.woozooo.com.evil.test/".parse().unwrap()));
}

#[test]
fn ilanzou_aes_ecb_matches_independent_python_vectors() {
    for (plain, expected) in [
        ("1700000000123", "0f0032b7d6d3bf786bbf75bd3cfba202"),
        ("123|", "d5d00a7edc829f0458983805b32d714a"),
        ("123|456", "f2865dcb6c364c3214eaddc9ea47051a"),
        (
            "123|1700000000123",
            "b037e67243e9f372d34370f81c949b03e4fb2d7484451d0fe557a0830dcb9fb9",
        ),
    ] {
        assert_eq!(super::ilanzou::aes_hex(plain), expected);
    }
}

#[tokio::test]
async fn uc_personal_and_share_download_keep_cookie_encoding_and_account_boundaries() {
    let raw = json!({"fid":"fid123","file_name":"sample.zip","size":513,"dir":false,"share_fid_token":"private-file-token"});
    let (session, server) = fixture(
        Provider::Uc,
        vec![
            json!({"success":true,"data":{"uid":"uc-account","nickname":"UC fixture"}}),
            json!({"code":0,"data":{"list":[raw.clone()]},"metadata":{"_total":1}}),
            json!({"code":0,"data":{"stoken":"private-share-token","title":"UC fixture share"}}),
            json!({"code":0,"data":{"detail_info":{"list":[raw]}}}),
            json!({"code":0,"data":[{"download_url":"https://uc-cdn.aliyuncs.com/file"}]}),
            json!({"code":0,"data":[{"download_url":"https://uc-cdn.aliyuncs.com/own"}]}),
        ],
    )
    .await;
    let api = "https://pc-api.uc.cn/".parse().unwrap();
    session
        .transport
        .jar
        .add_cookie_str("__pugs=token%2Fencoded%3D; Path=/", &api);
    session
        .transport
        .jar
        .add_cookie_str("other_login_secret=private; Path=/", &api);
    assert_eq!(session.cloud_identity().await.unwrap().1, "UC fixture");
    let personal = session.cloud_page("/", 1, None).await.unwrap();
    assert_eq!(personal.entries[0].file.path, "/fid123");
    let share = parse_share("https://drive.uc.cn/s/ucshare?pwd=Ab12", "").unwrap();
    let listing = session.list(&share, "0").await.unwrap();
    let encoded = serde_json::to_string(&listing).unwrap();
    assert!(!encoded.contains("private-"));
    let download = session
        .download(&share, &listing.files[0], &listing.stoken)
        .await
        .unwrap();
    assert!(download
        .headers
        .contains(&"Cookie: __pugs=token%2Fencoded%3D".into()));
    assert!(!download
        .headers
        .iter()
        .any(|s| s.contains("other_login_secret") || s.contains("Authorization")));
    let own_download = session.uc_download("fid123", None).await.unwrap();
    assert!(own_download.user_agent.contains("uc-cloud-drive/1.6.1"));
    let requests = server.await.unwrap();
    assert!(requests[3]
        .target
        .starts_with("/1/clouddrive/transfer_share/detail?"));
    let body: Value = serde_json::from_str(&requests[4].body).unwrap();
    assert_eq!(body["fids_token"][0], "private-file-token");
    assert_eq!(body["stoken"], "private-share-token");
    assert_eq!(requests[4].headers["referer"], "https://drive.uc.cn/");
    assert_eq!(requests[3].headers["origin"], "https://fast.uc.cn");
    assert_eq!(requests[4].headers["user-agent"], download.user_agent);
    assert!(download.user_agent.contains("uc-cloud-drive/2.5.20"));
    assert!(requests[4].target.contains("entry=ft"));
    assert!(!requests[5].target.contains("entry="));
    assert!(requests[5].target.contains("sys=win32") && requests[5].target.contains("ve=1.6.1"));
}

#[tokio::test]
async fn ilanzou_lists_typed_ids_and_signs_own_and_anonymous_downloads() {
    let entries = json!([{"fileType":2,"folderId":12,"folderName":"folder"},{"fileType":1,"fileId":12,"fileName":"file.zip","fileSize":2}]);
    let(mut session,server)=fixture(Provider::Ilanzou,vec![
        json!({"code":200,"map":{"userId":456,"nickName":"fixture"}}),
        json!({"code":200,"list":entries,"totalPage":1}),
        json!({"code":200,"list":[{"fileType":1,"fileId":123,"fileName":"share.zip","fileSize":2}],"totalPage":1}),
        json!({"__fixture_status":302,"__fixture_body":"","__fixture_headers":{"Location":"https://files.qiniup.com/share.zip"}}),
        json!({"code":200,"map":{"userId":456,"nickName":"fixture"}}),
        json!({"code":200,"url":"https://files.qiniup.com/own.zip"}),
    ]).await;
    session.authorization = "fixture:token+encoded/value".into();
    assert_eq!(session.cloud_identity().await.unwrap().1, "fixture");
    let own = session.cloud_page("/", 1, None).await.unwrap();
    assert_eq!(own.entries[0].file.id, "d_12");
    assert_eq!(own.entries[1].file.id, "f_12");
    assert_eq!(own.entries[1].file.size, 2048);
    let share = parse_share("https://www.ilanzou.com/s/Fixture", "").unwrap();
    let listing = session.list(&share, "d_12").await.unwrap();
    session.authorization.clear();
    let download = session
        .download(&share, &listing.files[0], "")
        .await
        .unwrap();
    assert_eq!(download.url, "https://files.qiniup.com/share.zip");
    session.authorization = "fixture:token+encoded/value".into();
    let own_url = session.ilanzou_download("f_123", None).await.unwrap();
    assert!(own_url.ends_with("own.zip"));
    let requests = server.await.unwrap();
    assert!(requests[0]
        .target
        .contains("appToken=fixture:token%2Bencoded%2Fvalue"));
    let query = |r: &Request| {
        url::Url::parse(&format!("https://fixture.test{}", r.target))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>()
    };
    assert_eq!(query(&requests[2])["folderId"], "12");
    assert!(!query(&requests[2]).contains_key("appToken"));
    assert_eq!(
        query(&requests[3])["downloadId"],
        "d5d00a7edc829f0458983805b32d714a"
    );
    assert_eq!(
        query(&requests[5])["downloadId"],
        "f2865dcb6c364c3214eaddc9ea47051a"
    );
    assert!(!query(&requests[3]).contains_key("extra"));
}

fn html(body: &str) -> Value {
    json!({"__fixture_body":body,"__fixture_type":"text/html"})
}

#[tokio::test]
async fn ilanzou_logged_in_share_uses_current_user_without_sending_app_token_to_cdn() {
    let (session, server) = fixture(
        Provider::Ilanzou,
        vec![
            json!({"code":200,"map":{"userId":456,"nickName":"fixture"}}),
            json!({"code":200,"url":"https://files.qiniup.com/account-share.zip"}),
        ],
    )
    .await;
    let share = parse_share("https://www.ilanzou.com/s/Fixture", "").unwrap();
    let address = session
        .ilanzou_download("f_123", Some(&share))
        .await
        .unwrap();
    let download = session.download_descriptor(address).unwrap();
    assert!(download
        .headers
        .iter()
        .all(|v| !v.contains("fixture-bearer")
            && !v.starts_with("Cookie:")
            && !v.starts_with("Authorization:")));
    let requests = server.await.unwrap();
    assert!(requests[0].target.starts_with("/proved/user/account/map?"));
    assert!(requests[1]
        .target
        .contains("downloadId=f2865dcb6c364c3214eaddc9ea47051a"));
    assert!(!requests[1].target.contains("appToken"));
}

#[tokio::test]
async fn lanzou_single_file_uses_same_origin_iframe_and_checks_actual_download_size() {
    let page =
        r#"<title>sample.zip - 蓝奏云</title>文件大小：1.5 K<iframe src='/fn?file=123'></iframe>"#;
    let frame = r#"var wp_sign='sign-fixture'; var ajaxdata='ajax-fixture'; var kdns=0; var domain1='https://apifile.woozooo.com/ajaxm.php?file=123';"#;
    let(session,server)=fixture(Provider::Lanzou,vec![html(page),html(page),html(frame),json!({"zt":1,"dom":"http://node.panlanzou.com","url":"object-fixture"}),json!({"__fixture_body":"x","__fixture_status":206,"__fixture_type":"application/octet-stream","__fixture_headers":{"Content-Range":"bytes 0-0/1537"}})]).await;
    let share = parse_share("https://someone.lanzoub.com/iFixture", "").unwrap();
    let listing = session.list(&share, "0").await.unwrap();
    assert_eq!(listing.files[0].name, "sample.zip");
    assert_eq!(listing.files[0].size, 1536);
    let download = session
        .download(&share, &listing.files[0], "")
        .await
        .unwrap();
    assert_eq!(download.size, Some(1537));
    assert_eq!(
        download.url,
        "https://node.panlanzou.com/file/object-fixture"
    );
    assert!(download.headers.iter().any(|s| s == "Cookie: down_ip=1"));
    let requests = server.await.unwrap();
    assert_eq!(requests[2].target, "/fn?file=123");
    let fields = url::form_urlencoded::parse(requests[3].body.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(fields["sign"], "sign-fixture");
    assert_eq!(fields["websignkey"], "ajax-fixture");
    assert_eq!(fields["kd"], "0");
    assert_eq!(requests[4].headers["range"], "bytes=0-0");
}

#[tokio::test]
async fn lanzou_personal_folders_files_and_pagination_preserve_distinct_ids() {
    let page = "var url='/doupload.php?uid=1234'; var vei='fixture+/=';";
    let (session, server) = fixture(
        Provider::Lanzou,
        vec![
            html(page),
            html(page),
            json!({"zt":1,"text":[{"fol_id":9,"name":"Folder &amp; Test"}]}),
            json!({"zt":1,"text":[{"id":9,"name_all":"file.zip","size":"2 M"}]}),
            html(page),
            json!({"zt":2,"text":[]}),
        ],
    )
    .await;
    assert!(session.cloud_identity().await.is_ok());
    let first = session.cloud_page("/", 1, None).await.unwrap();
    assert_eq!(first.entries[0].file.id, "d_9");
    assert_eq!(first.entries[0].file.name, "Folder & Test");
    assert_eq!(first.entries[1].file.id, "f_9");
    assert_eq!(first.entries[1].file.size, 2097152);
    assert!(first.next.is_some());
    assert!(session
        .cloud_page("/", 2, first.next.as_deref())
        .await
        .unwrap()
        .next
        .is_none());
    let requests = server.await.unwrap();
    assert!(requests[2].target.contains("vei=fixture%2B%2F%3D"));
    assert!(requests[2].body.contains("folder_id=-1"));
    assert!(requests[3].body.contains("task=5"));
}

#[tokio::test]
async fn lanzou_folder_passwords_and_wrong_iframe_domains_are_rejected() {
    let folder = "var lx=2; var fid=123; var t='time'; var k='key'; filemoreajax";
    let (session, server) = fixture(
        Provider::Lanzou,
        vec![
            html(folder),
            json!({"zt":1,"text":[{"id":"iFixture","name_all":"file.zip","size":"1 K"}]}),
            json!({"zt":2}),
            html(folder),
            json!({"zt":3}),
            html("<iframe src='https://evil.test/steal'></iframe>"),
        ],
    )
    .await;
    let share = parse_share("https://someone.lanzoub.com/bFolder 密码：123", "").unwrap();
    let listing = session.list(&share, "0").await.unwrap();
    assert_eq!(listing.files[0].id, "iFixture");
    assert!(session
        .list(&share, "0")
        .await
        .unwrap_err()
        .contains("提取码"));
    assert!(session
        .download(&share, &listing.files[0], "")
        .await
        .err()
        .unwrap()
        .contains("来源不一致"));
    let requests = server.await.unwrap();
    assert!(requests[1].body.contains("pwd=123"));
    assert_eq!(requests.len(), 6);
}

#[tokio::test]
async fn new_provider_redirects_and_credential_headers_do_not_cross_domains() {
    let(mut session,server)=fixture(Provider::Ilanzou,vec![json!({"__fixture_status":302,"__fixture_body":"","__fixture_headers":{"Location":"https://evil.test/leak"}})]).await;
    session.authorization.clear();
    assert!(session
        .ilanzou_download(
            "f_123",
            Some(&ShareLink {
                provider: Provider::Ilanzou,
                id: "share".into(),
                passcode: String::new()
            })
        )
        .await
        .is_err());
    assert_eq!(server.await.unwrap().len(), 1);
    for (provider, good) in [
        (Provider::Uc, "https://uc.aliyuncs.com/file"),
        (Provider::Ilanzou, "https://files.qiniup.com/file"),
        (Provider::Lanzou, "https://node.panlanzou.com/file"),
    ] {
        assert!(validate_download(provider, good).is_ok());
        for bad in [
            "https://127.0.0.1/file",
            "https://qiniup.com.evil.test/file",
            "https://lanzou.com.evil.test/file",
            "https://evil.test/?host=uc.cn",
        ] {
            assert!(validate_download(provider, bad).is_err());
        }
    }
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
            let response = if response["__fixture_download_by_id"] == true {
                let body: Value = serde_json::from_str(&request.body).unwrap();
                let id = body["fids"][0].as_str().unwrap();
                json!({"code":0,"data":[{"fid":id,"download_url":format!("https://cdn.quark.cn/{id}")}]})
            } else {
                response
            };
            requests.push(request);
            if response["__fixture_disconnect"] == true {
                continue;
            }
            let body = response["__fixture_body"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| response.to_string());
            let status = response["__fixture_status"].as_u64().unwrap_or(200);
            let content_type = response["__fixture_type"]
                .as_str()
                .unwrap_or("application/json");
            let mut extra = String::new();
            if let Some(headers) = response["__fixture_headers"].as_object() {
                for (key, value) in headers {
                    extra.push_str(&format!("{key}: {}\r\n", value.as_str().unwrap()));
                }
            }
            let headers = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\n{extra}Connection: close\r\n");
            let wire = if response["__fixture_chunked"] == true {
                let mut wire = format!("{headers}Transfer-Encoding: chunked\r\n\r\n");
                if !body.is_empty() {
                    wire.push_str(&format!("{:x}\r\n{body}\r\n", body.len()));
                }
                wire.push_str("0\r\n\r\n");
                wire
            } else {
                format!("{headers}Content-Length: {}\r\n\r\n{body}", body.len())
            };
            stream.write_all(wire.as_bytes()).await.unwrap();
        }
        requests
    });
    let mut transport = Quark::new().unwrap();
    transport.test_endpoint = Some(endpoint.clone());
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
async fn pan123_v2_uses_validated_candidate_nodes_and_falls_back_to_legacy_routes() {
    let file = ShareFile {
        id: "42".into(),
        name: "sample.bin".into(),
        size: 4,
        is_dir: false,
        parent_id: "0".into(),
        token: json!({"Etag":"checksum","S3KeyFlag":"object"}).to_string(),
    };
    let (session, server) = fixture(
        Provider::Pan123,
        vec![
            json!({"data":{"dispatchList":[
            {"prefix":"https://untrusted.example.org"},
            {"prefix":"https://cdn.123295.com/first"},
            {"prefix":"https://cdn.123295.com/second"}
        ],"downloadPath":"/file"}}),
            json!({"__fixture_status":503,"__fixture_body":"busy"}),
            json!({"__fixture_body":"data"}),
        ],
    )
    .await;
    assert_eq!(
        session.pan123_download(&file, None).await.unwrap(),
        "https://cdn.123295.com/second/file"
    );
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].target, "/api/v2/file/download_info");
    assert_eq!(requests[1].target, "/first/file");
    assert_eq!(requests[2].target, "/second/file");
    assert!(requests[1..]
        .iter()
        .all(|r| !r.headers.contains_key("authorization") && !r.headers.contains_key("cookie")));

    let share = parse_share("https://www.123pan.com/s/fixture", "").unwrap();
    let (session, server) = fixture(
        Provider::Pan123,
        vec![
            json!({"__fixture_status":404,"__fixture_body":"unsupported"}),
            json!({"code":0,"data":{"DownloadURL":"https://cdn.123295.com/legacy"}}),
            json!({"__fixture_body":"data"}),
        ],
    )
    .await;
    assert_eq!(
        session.pan123_download(&file, Some(&share)).await.unwrap(),
        "https://cdn.123295.com/legacy"
    );
    let requests = server.await.unwrap();
    assert_eq!(requests[0].target, "/api/v2/share/download/info");
    let v2_body: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(v2_body["S3keyFlag"], "object");
    assert_eq!(v2_body["FileID"], 42);
    assert!(v2_body.get("S3KeyFlag").is_none());
    assert_eq!(requests[1].target, "/b/api/share/download/info");
    let legacy_body: Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(legacy_body["S3KeyFlag"], "object");
    assert_eq!(legacy_body["FileID"], "42");
    assert_eq!(requests[1].headers["platform"], "android");
    for code in [5112, 5113] {
        let (session, server) = fixture(Provider::Pan123, vec![json!({"code":code})]).await;
        let error = session.pan123_download(&file, None).await.unwrap_err();
        assert!(error.contains(if code == 5112 { "登录" } else { "流量" }));
        assert_eq!(server.await.unwrap().len(), 1);
    }
    assert!(cloud::check(Provider::Pan123, json!({"code":"200"})).is_ok());
    assert!(cloud::check(Provider::Pan123, json!({"code":-1}))
        .unwrap_err()
        .contains("-1"));
}

#[tokio::test]
async fn quark_read_requests_recover_from_disconnects_but_preserve_denials() {
    let (session, server) = fixture(
        Provider::Quark,
        vec![
            json!({"__fixture_disconnect":true}),
            json!({"success":true,"data":{"nickname":"fixture"}}),
            json!({"__fixture_disconnect":true}),
            json!({"code":0,"data":{"token":"grant","expired_timestamp":(now()+600)*1000}}),
            json!({"__fixture_disconnect":true}),
            json!({"__fixture_download_by_id":true}),
            json!({"code":23018,"message":"permission denied"}),
        ],
    )
    .await;
    assert_eq!(session.account().await.unwrap(), "fixture");
    assert_eq!(
        session
            .transport
            .download_file("first", None)
            .await
            .unwrap(),
        "https://cdn.quark.cn/first"
    );
    assert!(session
        .transport
        .download_file("denied", None)
        .await
        .unwrap_err()
        .contains("游客"));
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 7);
    assert!(requests[0].target.contains("account/info"));
    assert_eq!(requests[0].target, requests[1].target);
    assert_eq!(requests[2].target, requests[3].target);
    assert_eq!(requests[4].body, requests[5].body);
}

#[tokio::test]
async fn quark_concurrent_downloads_share_a_grant_and_refresh_an_expired_grant_once() {
    let by_id = json!({"__fixture_download_by_id":true});
    let expiration = (now() + 600) * 1000;
    let (session, server) = fixture(
        Provider::Quark,
        vec![
            json!({"code":0,"data":{"token":"first","expired_timestamp":expiration}}),
            by_id.clone(),
            by_id.clone(),
            json!({"code":400,"message":"download token expired"}),
            json!({"code":0,"data":{"token":"second","expired_timestamp":expiration}}),
            by_id,
        ],
    )
    .await;
    let (first, second) = tokio::join!(
        session.transport.download_file("a", None),
        session.transport.download_file("b", None),
    );
    assert_eq!(first.unwrap(), "https://cdn.quark.cn/a");
    assert_eq!(second.unwrap(), "https://cdn.quark.cn/b");
    assert_eq!(
        session.transport.download_file("c", None).await.unwrap(),
        "https://cdn.quark.cn/c"
    );
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests[0].target.contains("acquire_dl_token"));
    assert!(requests[4].target.contains("acquire_dl_token"));
    for index in [1, 2, 3] {
        assert_eq!(
            serde_json::from_str::<Value>(&requests[index].body).unwrap()["token"],
            "first"
        );
    }
    assert_eq!(
        serde_json::from_str::<Value>(&requests[5].body).unwrap()["token"],
        "second"
    );
}

#[tokio::test]
async fn quark_grants_expire_early_and_are_isolated_between_browser_sessions() {
    let by_id = json!({"__fixture_download_by_id":true});
    let grant = |token: &str, seconds| json!({"code":0,"data":{"token":token,"expired_timestamp":(now()+seconds)*1000}});
    let (session, server) = fixture(
        Provider::Quark,
        vec![
            grant("nearly-expired", 30),
            by_id.clone(),
            grant("valid", 600),
            by_id.clone(),
            grant("new-session", 600),
            by_id,
        ],
    )
    .await;
    session.transport.download_file("a", None).await.unwrap();
    session.transport.download_file("b", None).await.unwrap();
    let mut other = Quark::new().unwrap();
    other.test_endpoint = session.test_endpoint.clone();
    other.download_file("c", None).await.unwrap();
    let requests = server.await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.target.contains("acquire_dl_token"))
            .count(),
        3
    );
    assert_eq!(
        serde_json::from_str::<Value>(&requests[5].body).unwrap()["token"],
        "new-session"
    );
}

#[tokio::test]
async fn quark_downloads_keep_share_credentials_and_reject_mismatched_files() {
    let (session, server) = fixture(Provider::Quark, vec![
        json!({"code":0,"data":{"token":"grant","expired_timestamp":(now()+600)*1000}}),
        json!({"code":0,"data":[{"fid":"a","download_url":"https://quark-cdn.aliyuncs.com/a"}]}),
        json!({"code":0,"data":[{"fid":"other","download_url":"https://cdn.quark.cn/other"}]}),
    ]).await;
    let api = "https://drive-pc.quark.cn/".parse().unwrap();
    session
        .transport
        .jar
        .add_cookie_str("__pugs=encoded%2Fsession; Path=/", &api);
    session
        .transport
        .jar
        .add_cookie_str("unrelated_login_secret=private; Path=/", &api);
    let share = parse_share("https://pan.quark.cn/s/fixture", "").unwrap();
    let file = ShareFile {
        id: "a".into(),
        name: "a.bin".into(),
        size: 4,
        is_dir: false,
        parent_id: "0".into(),
        token: "file-token".into(),
    };
    let download = session
        .download(&share, &file, "share-token")
        .await
        .unwrap();
    assert!(download
        .headers
        .contains(&"Cookie: __pugs=encoded%2Fsession".into()));
    assert!(!download
        .headers
        .iter()
        .any(|v| v.contains("private") || v.contains("unrelated")));
    assert!(session
        .transport
        .download_file("a", None)
        .await
        .unwrap_err()
        .contains("不匹配"));
    let requests = server.await.unwrap();
    let body: Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(body["fids_token"], json!(["file-token"]));
    assert_eq!(body["stoken"], "share-token");
    assert_eq!(body["pwd_id"], "fixture");
    assert_eq!(requests[1].headers["user-agent"], download.user_agent);
}

#[tokio::test]
async fn batch_cloud_lookup_reads_each_directory_once_and_preserves_file_mapping() {
    let a = json!({"fid":"a","file_name":"same.zip","size":11,"dir":false,"md5":"hash-a"});
    let b = json!({"fid":"b","file_name":"same.zip","size":22,"dir":false,"md5":"hash-b"});
    let (session, server) = fixture(
        Provider::Quark,
        vec![json!({"code":0,"data":{"list":[a.clone(),b.clone()]},"metadata":{"_total":2}})],
    )
    .await;
    let files = vec![
        cloud::entry(Provider::Quark, &b, "/").unwrap().file,
        cloud::entry(Provider::Quark, &a, "/").unwrap().file,
    ];
    let fresh = session.cloud_find_many(&files).await.unwrap();
    assert_eq!(fresh[0].file.id, "b");
    assert_eq!(fresh[0].file.size, 22);
    assert_eq!(fresh[1].file.id, "a");
    assert_eq!(fresh[1].file.md5, "hash-a");
    assert_eq!(server.await.unwrap().len(), 1);
    let wrong = OwnSource {
        account_id: "fixture".into(),
        file: files[0].clone(),
    };
    assert!(session
        .cloud_download_entry(&wrong, fresh[1].clone())
        .await
        .err()
        .unwrap()
        .contains("源文件已变化"));
}

#[tokio::test]
async fn quark_personal_download_refreshes_metadata_before_issuing_url() {
    let raw =
        json!({"fid":"q123","file_name":"fixture.txt","size":32,"dir":false,"md5":"checksum"});
    let (session, server) = fixture(
        Provider::Quark,
        vec![
            json!({"__fixture_disconnect":true}),
            json!({"success":true,"data":{"uid":"quark-fixture"}}),
            json!({"__fixture_disconnect":true}),
            json!({"code":0,"data":{"list":[raw.clone()]},"metadata":{"_total":1}}),
            json!({"code":0,"data":{"token":"download-fixture"}}),
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
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[0].target, requests[1].target);
    assert_eq!(requests[2].target, requests[3].target);
    assert!(requests[3].target.contains("pdir_fid=0"));
    assert_eq!(
        serde_json::from_str::<Value>(&requests[5].body).unwrap()["fids"],
        json!(["q123"])
    );
    assert!(requests[5].headers["user-agent"].contains("QuarkPC/"));
    assert_eq!(
        serde_json::from_str::<Value>(&requests[5].body).unwrap()["token"],
        "download-fixture"
    );
}

#[tokio::test]
async fn pan123_personal_download_follows_unmarked_redirects_without_api_credentials() {
    let raw = json!({"FileId":123,"FileName":"fixture.txt","Size":32,"Type":0,"Etag":"checksum","S3KeyFlag":"object"});
    let start = "https://download-cdn.cjjd19.com/lookup";
    let edge = "https://node.pd1.cjjd19.com:30443/routes/start";
    let target =
        "https://user-other-free-download-cdn.123295.com/files/fixture?params=opaque-signature";
    let (session, server) = fixture(
        Provider::Pan123,
        vec![
            json!({"code":0,"data":{"UID":1234,"Nickname":"fixture"}}),
            json!({"code":0,"data":{"InfoList":[raw.clone()],"Next":"-1"}}),
            json!({"code":0,"data":{"DownloadUrl": format!("https://www.123pan.com/download-v2?params={}", STANDARD.encode(start))}}),
            json!({"__fixture_chunked":true,"__fixture_body":json!({"code":0,"data":{"redirect_url":edge}}).to_string()}),
            json!({"__fixture_status":302,"__fixture_headers":{"Location":"../files/fixture?params=opaque-signature"},"__fixture_body":""}),
            json!({"__fixture_status":307,"__fixture_headers":{"Location":target},"__fixture_body":""}),
            json!({"__fixture_headers":{"Content-Disposition":"attachment; filename=fixture.txt"},"__fixture_body":r#"{"data":{"redirect_url":"should-not-follow"}}"#}),
        ],
    )
    .await;
    let source = OwnSource {
        account_id: crypto::md5_hex("pan123:1234"),
        file: cloud::entry(Provider::Pan123, &raw, "/").unwrap().file,
    };
    let download = session.cloud_download(&source).await.unwrap();
    assert_eq!(download.url, target);
    assert_eq!(download.headers.len(), 1);
    let requests = server.await.unwrap();
    for request in &requests[..2] {
        assert_eq!(request.headers["authorization"], "Bearer fixture-bearer");
        assert_eq!(request.headers["loginuuid"], session.device_id);
        assert!(
            request.headers.contains_key("auth-key") && request.headers.contains_key("auth-value")
        );
    }
    assert_eq!(requests[2].target, "/api/v2/file/download_info");
    assert_eq!(
        requests[2].headers["authorization"],
        "Bearer fixture-bearer"
    );
    assert_eq!(requests[3].target, "/lookup");
    assert_eq!(requests[4].target, "/routes/start");
    assert_eq!(requests[5].target, "/files/fixture?params=opaque-signature");
    assert_eq!(requests[6].target, "/files/fixture?params=opaque-signature");
    for request in &requests[3..] {
        assert_eq!(request.headers["referer"], "https://yun.123pan.cn/");
        assert_eq!(request.headers["range"], "bytes=0-8191");
        assert_eq!(request.headers["user-agent"], download.user_agent);
        for header in [
            "cookie",
            "authorization",
            "loginuuid",
            "auth-key",
            "auth-value",
        ] {
            assert!(!request.headers.contains_key(header));
        }
    }
    let payload: Value = serde_json::from_str(&requests[2].body).unwrap();
    assert_eq!(payload["fileId"], 123);
    assert_eq!(payload["s3keyFlag"], "object");
}

#[tokio::test]
async fn pan123_retries_interrupted_cdn_connections_but_stops_on_permission_errors() {
    let file = ShareFile {
        id: "43".into(),
        name: "fixture.bin".into(),
        size: 4,
        is_dir: false,
        parent_id: "0".into(),
        token: json!({"S3KeyFlag":"object", "Etag":"checksum"}).to_string(),
    };
    let node = "https://user-other-free-download-cdn.123295.com/fixture";
    let info = json!({"code":0, "data":{"DownloadURL":node}});
    let dropped = json!({"__fixture_disconnect":true});
    for (replies, expected, cdn_requests) in [
        (
            vec![dropped.clone(), json!({"__fixture_body":"data"})],
            Ok(node),
            2,
        ),
        (
            vec![dropped.clone(), dropped.clone(), dropped],
            Err("连接失败"),
            3,
        ),
        (
            vec![json!({"__fixture_status":403,"__fixture_body":""})],
            Err("HTTP 403"),
            1,
        ),
    ] {
        let (session, server) = fixture(
            Provider::Pan123,
            [
                vec![json!({"__fixture_disconnect":true}), info.clone()],
                replies,
            ]
            .concat(),
        )
        .await;
        let result = session.pan123_download(&file, None).await;
        match expected {
            Ok(url) => assert_eq!(result.unwrap(), url),
            Err(reason) => assert!(result.unwrap_err().contains(reason)),
        }
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2 + cdn_requests);
        assert_eq!(requests[0].target, requests[1].target);
        assert_eq!(requests[0].body, requests[1].body);
        for request in &requests[2..] {
            assert_eq!(request.target, "/fixture");
            assert_eq!(request.headers["range"], "bytes=0-8191");
            assert!(
                !request.headers.contains_key("authorization")
                    && !request.headers.contains_key("cookie")
            );
        }
    }
}

#[tokio::test]
async fn pan123_bounds_redirects_and_retries_a_refused_range() {
    let file = ShareFile {
        id: "41".into(),
        name: "fixture".into(),
        size: 12000,
        is_dir: false,
        parent_id: "0".into(),
        token: json!({"S3KeyFlag":"object","Etag":"checksum"}).to_string(),
    };
    let start = "https://cdn.123pancloud.com/start";
    for hops in [5, 6] {
        let mut replies = vec![json!({"code":0,"data":{"DownloadUrl":start}})];
        for n in 1..=hops {
            replies.push(json!({"__fixture_status":307,"__fixture_headers":{"Location":format!("/step/{n}")},"__fixture_body":""}));
        }
        if hops == 5 {
            replies.push(json!({"__fixture_status":416,"__fixture_body":""}));
            // Even a redirect-shaped prefix is file content once the unframed body exceeds 8 KiB.
            let body = format!(
                r#"{{"data":{{"redirect_url":"/wrong"}}}}{}"#,
                " ".repeat(12000)
            );
            replies.push(json!({"__fixture_chunked":true,"__fixture_body":body}));
        }
        let (session, server) = fixture(Provider::Pan123, replies).await;
        let result = session.pan123_download(&file, None).await;
        let requests = server.await.unwrap();
        if hops == 5 {
            assert_eq!(result.unwrap(), "https://cdn.123pancloud.com/step/5");
            assert_eq!(requests.last().unwrap().target, "/step/5");
            assert!(!requests.last().unwrap().headers.contains_key("range"));
        } else {
            assert!(result.unwrap_err().contains("跳转次数过多"));
            assert_eq!(requests.len(), 7);
        }
    }
}

#[tokio::test]
async fn pan123_share_checks_every_cdn_hop_and_preserves_small_json_files() {
    let share = parse_share("https://www.123295.com/s/fixture", "").unwrap();
    let body = r#"{"code":0,"data":{"redirect_url":"https://another.example.org/content"}}"#;
    let file = ShareFile {
        id: "42".into(),
        name: "fixture.json".into(),
        size: body.len() as u64,
        is_dir: false,
        parent_id: "0".into(),
        token: json!({"S3KeyFlag":"object","Etag":crypto::md5_hex(body)}).to_string(),
    };
    let start = "https://user-other-free-download-cdn.123295.com/share";
    let download_info = json!({"code":0,"data":{"DownloadURL":start}});
    let (session, server) = fixture(Provider::Pan123, vec![
        download_info.clone(),
        json!({"code":0,"data":{"redirect_url":"https://unknown.example.org:9443/path?sig=private"}}),
    ]).await;
    let error = session.download(&share, &file, "").await.err().unwrap();
    assert!(error.contains("unknown.example.org:9443") && !error.contains("private"));
    assert_eq!(server.await.unwrap().len(), 2);

    let (session, server) = fixture(
        Provider::Pan123,
        vec![
            download_info,
            json!({"__fixture_chunked":true,"__fixture_body":body}),
        ],
    )
    .await;
    assert_eq!(
        session.download(&share, &file, "").await.unwrap().url,
        start
    );
    assert_eq!(server.await.unwrap().len(), 2);
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
