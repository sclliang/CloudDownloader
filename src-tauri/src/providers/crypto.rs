// Protocol constants and independent Rust implementations of the wire formats.
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use md5::{Digest, Md5};
use num_bigint::BigUint;
use std::io::Read;
const KEY_TABLE: [u8; 144] = [
    0xf0, 0xe5, 0x69, 0xae, 0xbf, 0xdc, 0xbf, 0x8a, 0x1a, 0x45, 0xe8, 0xbe, 0x7d, 0xa6, 0x73, 0xb8,
    0xde, 0x8f, 0xe7, 0xc4, 0x45, 0xda, 0x86, 0xc4, 0x9b, 0x64, 0x8b, 0x14, 0x6a, 0xb4, 0xf1, 0xaa,
    0x38, 0x01, 0x35, 0x9e, 0x26, 0x69, 0x2c, 0x86, 0x00, 0x6b, 0x4f, 0xa5, 0x36, 0x34, 0x62, 0xa6,
    0x2a, 0x96, 0x68, 0x18, 0xf2, 0x4a, 0xfd, 0xbd, 0x6b, 0x97, 0x8f, 0x4d, 0x8f, 0x89, 0x13, 0xb7,
    0x6c, 0x8e, 0x93, 0xed, 0x0e, 0x0d, 0x48, 0x3e, 0xd7, 0x2f, 0x88, 0xd8, 0xfe, 0xfe, 0x7e, 0x86,
    0x50, 0x95, 0x4f, 0xd1, 0xeb, 0x83, 0x26, 0x34, 0xdb, 0x66, 0x7b, 0x9c, 0x7e, 0x9d, 0x7a, 0x81,
    0x32, 0xea, 0xb6, 0x33, 0xde, 0x3a, 0xa9, 0x59, 0x34, 0x66, 0x3b, 0xaa, 0xba, 0x81, 0x60, 0x48,
    0xb9, 0xd5, 0x81, 0x9c, 0xf8, 0x6c, 0x84, 0x77, 0xff, 0x54, 0x78, 0x26, 0x5f, 0xbe, 0xe8, 0x1e,
    0x36, 0x9f, 0x34, 0x80, 0x5c, 0x45, 0x2c, 0x9b, 0x76, 0xd5, 0x1b, 0x8f, 0xcc, 0xc3, 0xb8, 0xf5,
];
const MODULUS: &str = "8686980c0f5a24c4b9d43020cd2c22703ff3f450756529058b1cf88f09b8602136477198a6e2683149659bd122c33592fdb5ad47944ad1ea4d36c6b172aad6338c3bb6ac6227502d010993ac967d1aef00f0c8e038de2e4d3bc2ec368af2e9f10a6f1eda4f7262f136420c07c331b871bf139f74f3010e3c4fe57df3afb71683";
const LONG_KEY: [u8; 12] = [
    0x78, 0x06, 0xad, 0x4c, 0x33, 0x86, 0x5d, 0x18, 0x4c, 0x01, 0x3f, 0x46,
];
const JS_ESCAPE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(39)
    .remove(b'(')
    .remove(b')');
pub fn md5_hex(value: &str) -> String {
    format!("{:x}", Md5::digest(value.as_bytes()))
}
pub fn base64_decode(value: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(value)
        .or_else(|_| URL_SAFE.decode(value))
        .or_else(|_| URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')))
        .map_err(|_| "网盘返回的编码数据无效".into())
}
pub fn sign123(path: &str, seconds: i64, random: u32) -> (String, String) {
    let time = chrono::DateTime::from_timestamp(seconds + 57600, 0)
        .unwrap()
        .format("%Y%m%d%H%M")
        .to_string();
    let table = b"adefghlmyijnopkqrstubcvwsz";
    let mapped: Vec<_> = time.bytes().map(|c| table[(c - b'0') as usize]).collect();
    let key = format!("{:x}", crc32fast::hash(&mapped));
    let input = format!("{seconds}|{random}|{path}|web|3|{key}");
    (
        key,
        format!("{seconds}-{random}-{:x}", crc32fast::hash(input.as_bytes())),
    )
}
pub fn sign139(body: &str, timestamp: &str, random: &str) -> String {
    let encoded = percent_encoding::utf8_percent_encode(body, JS_ESCAPE).to_string();
    let mut sorted = encoded.into_bytes();
    sorted.sort_unstable();
    md5_hex(&(md5_hex(&STANDARD.encode(sorted)) + &md5_hex(&format!("{timestamp}:{random}"))))
        .to_ascii_uppercase()
}
pub fn encrypt139(body: &str) -> Result<String, String> {
    let mut iv = [0u8; 16];
    getrandom::fill(&mut iv).map_err(|_| "无法生成请求随机数")?;
    let ciphertext = cbc::Encryptor::<aes::Aes128>::new(b"PVGDwmcvfs1uV3d1".into(), (&iv).into())
        .encrypt_padded_vec_mut::<Pkcs7>(body.as_bytes());
    let mut data = iv.to_vec();
    data.extend(ciphertext);
    Ok(STANDARD.encode(data))
}
pub fn decrypt139(body: &str) -> Result<serde_json::Value, String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if value.is_object() {
            return Ok(value);
        }
    }
    let data = base64_decode(body.trim().trim_matches('"'))?;
    if data.len() < 32 {
        return Err("139 返回的加密响应不完整".into());
    }
    let plain = cbc::Decryptor::<aes::Aes128>::new(b"PVGDwmcvfs1uV3d1".into(), data[..16].into())
        .decrypt_padded_vec_mut::<Pkcs7>(&data[16..])
        .map_err(|_| "139 响应解密失败")?;
    let plain = if plain.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(plain.as_slice())
            .take(8 * 1024 * 1024)
            .read_to_end(&mut out)
            .map_err(|_| "139 响应解压失败")?;
        out
    } else {
        plain
    };
    serde_json::from_slice(&plain).map_err(|_| "139 响应内容无效".into())
}
fn derived(seed: &[u8], length: usize) -> Vec<u8> {
    (0..length)
        .map(|i| {
            seed[i % seed.len()].wrapping_add(KEY_TABLE[length * i])
                ^ KEY_TABLE[length * (length - 1 - i)]
        })
        .collect()
}
fn xor(data: &mut [u8], key: &[u8]) {
    let prefix = data.len() % 4;
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[if i < prefix {
            i % key.len()
        } else {
            (i - prefix) % key.len()
        }];
    }
}
fn rsa(block: &[u8]) -> Result<Vec<u8>, String> {
    let modulus = BigUint::parse_bytes(MODULUS.as_bytes(), 16).unwrap();
    let value = BigUint::from_bytes_be(block);
    if value >= modulus {
        return Err("115 返回的 RSA 数据无效".into());
    }
    let bytes = value
        .modpow(&BigUint::from(65537u32), &modulus)
        .to_bytes_be();
    let mut out = vec![0; 128 - bytes.len()];
    out.extend(bytes);
    Ok(out)
}
pub fn encode115(payload: &str, key: &[u8; 16]) -> Result<String, String> {
    let mut body = payload.as_bytes().to_vec();
    xor(&mut body, &derived(key, 4));
    body.reverse();
    xor(&mut body, &LONG_KEY);
    let mut plain = key.to_vec();
    plain.extend(body);
    let mut output = Vec::new();
    for chunk in plain.chunks(117) {
        let mut block = [255u8; 128];
        block[0] = 0;
        block[1] = 2;
        let start = 127 - chunk.len();
        block[start] = 0;
        block[start + 1..].copy_from_slice(chunk);
        output.extend(rsa(&block)?);
    }
    Ok(STANDARD.encode(output))
}
pub fn decode115(payload: &str, key: &[u8; 16]) -> Result<serde_json::Value, String> {
    let ciphertext = base64_decode(payload)?;
    if ciphertext.is_empty() || ciphertext.len() % 128 != 0 {
        return Err("115 返回的 RSA 响应长度无效".into());
    }
    let mut output = Vec::new();
    for chunk in ciphertext.chunks_exact(128) {
        let block = rsa(chunk)?;
        let at = block[2..]
            .iter()
            .position(|c| *c == 0)
            .map(|p| p + 3)
            .filter(|p| *p >= 11)
            .ok_or("115 响应填充无效")?;
        output.extend_from_slice(&block[at..]);
    }
    if output.len() <= 16 {
        return Err("115 返回的加密响应不完整".into());
    }
    let (salt, body) = output.split_at_mut(16);
    xor(body, &derived(salt, 12));
    body.reverse();
    xor(body, &derived(key, 4));
    serde_json::from_slice(body).map_err(|_| "115 响应内容无效".into())
}
