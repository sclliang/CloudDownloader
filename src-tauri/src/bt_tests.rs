use super::*;

pub fn single(name: &str, payload: &[u8]) -> Vec<u8> {
    let pieces: Vec<_> = payload
        .chunks(16384)
        .flat_map(|p| Sha1::digest(p).to_vec())
        .collect();
    let mut info = format!(
        "d6:lengthi{}e4:name{}:{}12:piece lengthi16384e6:pieces{}:",
        payload.len(),
        name.len(),
        name,
        pieces.len()
    )
    .into_bytes();
    info.extend(pieces);
    info.push(b'e');
    let mut data = b"d4:info".to_vec();
    data.extend(info);
    data.push(b'e');
    data
}

#[test]
fn torrent_hash_uses_original_info_bytes_and_rejects_malformed_paths() {
    let data = single("hello.txt", b"hello world");
    let torrent = parse(&data).unwrap();
    assert_eq!(
        torrent.files,
        vec![BtFile {
            index: 1,
            path: "hello.txt".into(),
            size: 11
        }]
    );
    assert_eq!(
        torrent.info_hash,
        format!("{:x}", Sha1::digest(&data[7..data.len() - 1]))
    );
    for name in ["../escape", "CON.txt", "a:b", "x\\y", ".", "test.", ""] {
        assert!(parse(&single(name, b"x")).is_err(), "{name}");
    }
    for cut in 0..data.len() {
        assert!(parse(&data[..cut]).is_err());
    }
    let mut extra = data.clone();
    extra.push(0);
    assert!(parse(&extra).is_err());
    assert!(parse(b"d4:infod12:meta versioni2e4:name1:xee")
        .unwrap_err()
        .contains("v2"));
}

#[test]
fn manifest_rejects_case_collisions_and_file_folder_conflicts() {
    let file = |index, path: &str| BtFile {
        index,
        path: path.into(),
        size: 1,
    };
    assert!(validate_files(&[file(1, "root/A"), file(2, "root/a")]).is_err());
    assert!(validate_files(&[file(1, "root/a"), file(2, "root/a/b")]).is_err());
    assert!(validate_files(&[file(1, "root/a"), file(2, "root/b")]).is_ok());
}

#[test]
fn magnet_accepts_hex_and_base32_and_rejects_unsupported_hashes() {
    let (hash, link) = magnet("magnet:?xt=urn:btih:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&dn=test&tr=udp%3A%2F%2F127.0.0.1%3A1234&ws=https://evil.com/x").unwrap();
    assert_eq!(hash, "0".repeat(40));
    assert!(!link.contains("evil"));
    assert_eq!(
        magnet(&format!("magnet:?xt=urn:btih:{}", "F".repeat(40)))
            .unwrap()
            .0,
        "f".repeat(40)
    );
    for bad in [
        "magnet:?xt=urn:btmh:1220ffff",
        "magnet:?xt=urn:btih:abcd",
        "https://example.com/x",
    ] {
        assert!(magnet(bad).is_err());
    }
}

#[test]
fn cache_is_bounded_and_verified_on_every_read() {
    let base =
        std::env::temp_dir().join(format!("clouddownloader-bt-cache-{}", uuid::Uuid::new_v4()));
    let data = single("a.txt", b"abc");
    let torrent = cache(&base, &data).unwrap();
    assert_eq!(load(&base, &torrent.info_hash).unwrap().1, data);
    cache(&base, &data).unwrap();
    let path = cache_path(&base, &torrent.info_hash).unwrap();
    fs::write(&path, single("a.txt", b"xyz")).unwrap();
    assert!(load(&base, &torrent.info_hash).is_err());
    assert!(cache_path(&base, "../escape").is_err());
    fs::remove_file(path).unwrap();
    fs::remove_dir(base.join("bt-torrents")).unwrap();
    fs::remove_dir(base).unwrap();
}
