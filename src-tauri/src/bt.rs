use crate::model::{safe_name, BtFile, Task};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub const MAX_TORRENT: usize = 4 * 1024 * 1024;
const MARKER: &str = ".clouddownloader-owner";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Torrent {
    pub info_hash: String,
    pub piece_length: u64,
    pub name: String,
    pub files: Vec<BtFile>,
}

#[derive(Debug)]
enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Node>),
    Dict(BTreeMap<Vec<u8>, Node>),
}
#[derive(Debug)]
struct Node {
    start: usize,
    end: usize,
    value: Value,
}
impl Node {
    fn get(&self, key: &[u8]) -> Result<&Node, String> {
        if let Value::Dict(v) = &self.value {
            v.get(key).ok_or_else(|| "种子缺少必要字段".into())
        } else {
            Err("种子字典格式无效".into())
        }
    }
    fn optional(&self, key: &[u8]) -> Option<&Node> {
        self.get(key).ok()
    }
    fn bytes(&self) -> Result<&[u8], String> {
        if let Value::Bytes(v) = &self.value {
            Ok(v)
        } else {
            Err("种子文本字段无效".into())
        }
    }
    fn number(&self) -> Result<u64, String> {
        if let Value::Int(v) = self.value {
            u64::try_from(v).map_err(|_| "种子长度无效".into())
        } else {
            Err("种子数字字段无效".into())
        }
    }
    fn list(&self) -> Result<&[Node], String> {
        if let Value::List(v) = &self.value {
            Ok(v)
        } else {
            Err("种子文件列表无效".into())
        }
    }
    fn text(&self) -> Result<String, String> {
        String::from_utf8(self.bytes()?.to_vec())
            .map_err(|_| "种子文件名编码不支持，请使用 UTF-8 种子".into())
    }
}

fn node(data: &[u8], pos: &mut usize, depth: usize, count: &mut usize) -> Result<Node, String> {
    *count += 1;
    if depth > 40 || *count > 100_000 {
        return Err("种子结构过于复杂".into());
    }
    let start = *pos;
    let byte = *data.get(*pos).ok_or("种子内容不完整")?;
    let value = match byte {
        b'i' => {
            *pos += 1;
            let end = data[*pos..]
                .iter()
                .position(|b| *b == b'e')
                .ok_or("种子数字不完整")?
                + *pos;
            let text = std::str::from_utf8(&data[*pos..end]).map_err(|_| "种子数字无效")?;
            if text.is_empty()
                || text == "-0"
                || text.starts_with('+')
                || (text.starts_with('0') && text.len() > 1)
                || text.starts_with("-0")
            {
                return Err("种子数字编码无效".into());
            }
            let v = text.parse().map_err(|_| "种子数字超出范围")?;
            *pos = end + 1;
            Value::Int(v)
        }
        b'l' | b'd' => {
            *pos += 1;
            let mut list = Vec::new();
            let mut dict = BTreeMap::new();
            let mut last: Option<Vec<u8>> = None;
            while data.get(*pos) != Some(&b'e') {
                if byte == b'l' {
                    list.push(node(data, pos, depth + 1, count)?);
                } else {
                    let key = node(data, pos, depth + 1, count)?.bytes()?.to_vec();
                    if last.as_ref().is_some_and(|old| old >= &key) {
                        return Err("种子字典字段重复或顺序无效".into());
                    }
                    last = Some(key.clone());
                    dict.insert(key, node(data, pos, depth + 1, count)?);
                }
            }
            *pos += 1;
            if byte == b'l' {
                Value::List(list)
            } else {
                Value::Dict(dict)
            }
        }
        b'0'..=b'9' => {
            let colon = data[*pos..]
                .iter()
                .position(|b| *b == b':')
                .ok_or("种子字符串不完整")?
                + *pos;
            if colon - *pos > 8 || (data[*pos] == b'0' && colon > *pos + 1) {
                return Err("种子字符串长度无效".into());
            }
            let len: usize = std::str::from_utf8(&data[*pos..colon])
                .map_err(|_| "种子长度无效")?
                .parse()
                .map_err(|_| "种子长度无效")?;
            let end = (colon + 1)
                .checked_add(len)
                .filter(|v| *v <= data.len())
                .ok_or("种子字符串被截断")?;
            *pos = end;
            Value::Bytes(data[colon + 1..end].to_vec())
        }
        _ => return Err("不是有效的 BitTorrent 种子文件".into()),
    };
    Ok(Node {
        start,
        end: *pos,
        value,
    })
}

pub fn component(text: &str) -> Result<(), String> {
    let stem = text.split('.').next().unwrap_or_default().to_uppercase();
    if text.is_empty()
        || text == "."
        || text == ".."
        || safe_name(text) != text
        || text.len() > 180
        || text.eq_ignore_ascii_case(MARKER)
        || matches!(stem.as_str(), "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|s| matches!(s, "¹" | "²" | "³"))
        })
    {
        return Err("种子包含 Windows 不支持或不安全的文件路径".into());
    }
    Ok(())
}

pub fn parse(data: &[u8]) -> Result<Torrent, String> {
    if data.is_empty() || data.len() > MAX_TORRENT {
        return Err("种子文件应小于 4 MiB 且不能为空".into());
    }
    let mut pos = 0;
    let root = node(data, &mut pos, 0, &mut 0)?;
    if pos != data.len() {
        return Err("种子包含多余内容".into());
    }
    let info = root.get(b"info")?;
    if info.optional(b"pieces").is_none() {
        return Err("暂不支持纯 BT v2 种子，请使用 BT v1 或混合种子".into());
    }
    let name = info
        .optional(b"name.utf-8")
        .unwrap_or(info.get(b"name")?)
        .text()?;
    component(&name)?;
    let mut files = Vec::new();
    if let Some(entries) = info.optional(b"files") {
        if info.optional(b"length").is_some() {
            return Err("种子同时包含单文件和多文件结构".into());
        }
        for entry in entries.list()? {
            if files.len() >= 10000 {
                return Err("种子文件超过 10000 项，请拆分任务".into());
            }
            if entry
                .optional(b"attr")
                .is_some_and(|n| n.bytes().is_ok_and(|b| b.contains(&b'l')))
                || entry.optional(b"symlink path").is_some()
            {
                return Err("不支持包含符号链接的种子".into());
            }
            let parts = entry
                .optional(b"path.utf-8")
                .unwrap_or(entry.get(b"path")?)
                .list()?;
            if parts.is_empty() {
                return Err("种子文件路径为空".into());
            }
            let mut path = name.clone();
            for part in parts {
                let part = part.text()?;
                component(&part)?;
                path.push('/');
                path.push_str(&part);
            }
            if path.len() > 220 {
                return Err("种子文件路径过长".into());
            }
            files.push(BtFile {
                index: files.len() as u32 + 1,
                path,
                size: entry.get(b"length")?.number()?,
            });
        }
    } else {
        if info
            .optional(b"attr")
            .is_some_and(|n| n.bytes().is_ok_and(|b| b.contains(&b'l')))
        {
            return Err("不支持符号链接种子".into());
        }
        files.push(BtFile {
            index: 1,
            path: name.clone(),
            size: info.get(b"length")?.number()?,
        });
    }
    validate_files(&files)?;
    let total = files
        .iter()
        .try_fold(0u64, |sum, f| sum.checked_add(f.size))
        .filter(|v| *v > 0 && *v <= i64::MAX as u64)
        .ok_or("种子总大小无效")?;
    let piece = info.get(b"piece length")?.number()?;
    if piece == 0
        || piece > 64 * 1024 * 1024
        || info.get(b"pieces")?.bytes()?.len() as u64 != total.div_ceil(piece) * 20
    {
        return Err("种子的分片校验信息无效".into());
    }
    Ok(Torrent {
        info_hash: format!("{:x}", Sha1::digest(&data[info.start..info.end])),
        piece_length: piece,
        name,
        files,
    })
}

pub fn validate_files(files: &[BtFile]) -> Result<(), String> {
    if files.is_empty() || files.len() > 10000 {
        return Err("种子文件列表无效".into());
    }
    let mut names = HashSet::new();
    for (i, file) in files.iter().enumerate() {
        if file.index != i as u32 + 1 {
            return Err("种子文件序号无效".into());
        }
        for part in file.path.split('/') {
            component(part)?;
        }
        if !names.insert(file.path.to_lowercase()) {
            return Err("种子包含重复的文件路径".into());
        }
    }
    for name in &names {
        let mut prefix = name.as_str();
        while let Some((parent, _)) = prefix.rsplit_once('/') {
            if names.contains(parent) {
                return Err("种子的文件和文件夹名称冲突".into());
            }
            prefix = parent;
        }
    }
    Ok(())
}

pub fn magnet(input: &str) -> Result<(String, String), String> {
    if input.len() > 16384 {
        return Err("磁力链接过长".into());
    }
    let url = url::Url::parse(input).map_err(|_| "磁力链接格式无效")?;
    if url.scheme() != "magnet" || url.host_str().is_some() {
        return Err("磁力链接格式无效".into());
    }
    let mut hash = None;
    let mut trackers = Vec::new();
    let mut name = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "xt" => {
                if let Some(v) = value.strip_prefix("urn:btih:") {
                    let decoded = if v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
                        v.to_lowercase()
                    } else if v.len() == 32 {
                        let mut bits = 0u32;
                        let mut available = 0;
                        let mut bytes = Vec::new();
                        for ch in v.bytes() {
                            let n = match ch.to_ascii_uppercase() {
                                b'A'..=b'Z' => ch.to_ascii_uppercase() - b'A',
                                b'2'..=b'7' => ch - b'2' + 26,
                                _ => return Err("磁力哈希无效".into()),
                            };
                            bits = (bits << 5) | n as u32;
                            available += 5;
                            if available >= 8 {
                                available -= 8;
                                bytes.push((bits >> available) as u8);
                            }
                        }
                        bytes.iter().map(|b| format!("{b:02x}")).collect()
                    } else {
                        return Err("磁力哈希无效".into());
                    };
                    if hash.as_ref().is_some_and(|old| old != &decoded) {
                        return Err("磁力链接包含多个不同的哈希".into());
                    }
                    hash = Some(decoded);
                }
            }
            "tr" if trackers.len() < 32 => {
                if let Ok(tr) = url::Url::parse(&value) {
                    if matches!(tr.scheme(), "http" | "https" | "udp")
                        && tr.host_str().is_some()
                        && tr.username().is_empty()
                        && tr.password().is_none()
                    {
                        trackers.push(value.into_owned());
                    }
                }
            }
            "dn" => name = Some(value.into_owned()),
            _ => {}
        }
    }
    let hash = hash.ok_or("磁力链接需要 BT v1 的 btih 哈希，暂不支持纯 BT v2")?;
    let mut out = url::Url::parse("magnet:?").unwrap();
    {
        let mut q = out.query_pairs_mut();
        q.append_pair("xt", &format!("urn:btih:{hash}"));
        if let Some(name) = name {
            q.append_pair("dn", &name);
        }
        for tr in trackers {
            q.append_pair("tr", &tr);
        }
    }
    Ok((hash, out.to_string()))
}

pub fn cache_path(base: &Path, hash: &str) -> Result<PathBuf, String> {
    if hash.len() != 40
        || !hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("种子标识无效".into());
    }
    Ok(base.join("bt-torrents").join(format!("{hash}.torrent")))
}
pub fn cache(base: &Path, data: &[u8]) -> Result<Torrent, String> {
    let torrent = parse(data)?;
    let path = cache_path(base, &torrent.info_hash)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|_| "无法创建种子缓存目录")?;
    // Atomic replacement keeps a crash from corrupting cached metadata.
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&tmp, data).map_err(|_| "无法保存种子缓存")?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("无法更新种子缓存：{e}"));
    }
    Ok(torrent)
}
pub fn load(base: &Path, hash: &str) -> Result<(Torrent, Vec<u8>), String> {
    let path = cache_path(base, hash)?;
    let data = read(&path)?;
    let torrent = parse(&data)?;
    if torrent.info_hash != hash {
        return Err("种子缓存校验失败，请重新导入原始种子".into());
    }
    Ok((torrent, data))
}
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut data = Vec::new();
    fs::File::open(path)
        .map_err(|_| "无法读取种子文件，请重新导入")?
        .take(MAX_TORRENT as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| "种子读取失败")?;
    if data.len() > MAX_TORRENT {
        return Err("种子文件不能超过 4 MiB".into());
    }
    Ok(data)
}

pub fn plain_path(path: &Path) -> Result<(), String> {
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(m) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    m.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if m.file_type().is_symlink() || reparse {
                    return Err("BT 目录包含链接或重解析点，请选择普通本地文件夹".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法检查 BT 文件路径".into()),
        }
    }
    Ok(())
}
pub fn outputs(task: &Task) -> Result<Vec<PathBuf>, String> {
    let root = Path::new(&task.directory);
    let source = task.bt.as_ref().ok_or("不是 BT 任务")?;
    component(&task.name)?;
    validate_files(&source.files)?;
    if !root.is_absolute() {
        return Err("BT 保存目录无效".into());
    }
    let mut files: Vec<_> = source.files.iter().map(|f| root.join(&f.path)).collect();
    files.push(root.join(format!("{}.aria2", task.name)));
    Ok(files)
}
pub fn owner_text(task: &Task) -> String {
    format!("{}:{}", task.id, task.bt.as_ref().unwrap().info_hash)
}
pub fn mark(task: &Task) -> Result<(), String> {
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(Path::new(&task.directory).join(MARKER))
        .and_then(|mut f| f.write_all(owner_text(task).as_bytes()))
        .map_err(|_| "无法记录 BT 任务目录归属".into())
}
pub fn validate_output(task: &Task) -> Result<(), String> {
    let root = Path::new(&task.directory);
    plain_path(root)?;
    let marker = root.join(MARKER);
    plain_path(&marker)?;
    if fs::read_to_string(&marker).ok().as_deref() != Some(&owner_text(task)) {
        return Err("BT 任务目录归属校验失败，已保留本地文件".into());
    }
    for file in outputs(task)? {
        plain_path(&file)?;
        if file.exists() && !file.is_file() {
            return Err("BT 文件位置被同名文件夹占用".into());
        }
    }
    Ok(())
}
pub fn marker(root: &Path) -> PathBuf {
    root.join(MARKER)
}

#[cfg(test)]
#[path = "bt_tests.rs"]
mod tests;
