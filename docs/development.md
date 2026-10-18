# 开发与构建

以下命令均在仓库根目录执行。

环境：Windows x64、Node.js、Rust MSVC、Visual Studio C++ Build Tools、WebView2 Runtime。

```powershell
npm ci
npm run tauri dev -- --no-watch
```

实际账号验证时使用 `--no-watch`，避免编辑 Rust 源码时重启登录窗口。普通开发可移除该选项。

从托盘菜单退出开发版后运行：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build.ps1
```

脚本执行格式检查、Rust 测试、前端和 Tauri NSIS 发布构建，并生成中文安装包、便携版 ZIP 和 SHA-256 校验文件。所有产物位于 `package/`；主程序为 `package/CloudDownloader.exe`，引擎与许可资料位于 `package/bin/`。安装版采用相同的 bin 布局，安装与升级保留原账号资料和任务。卸载默认保留这些资料，只有主动勾选清除应用数据才会删除。打包脚本核对主程序及安装包的公司、版权、英文功能说明、四段版本、原始文件名和英语（美国）资源语言。

```powershell
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --check
```

测试包含真实 aria2 与本地 HTTP 服务的多连接并发、暂停、引擎重启、更换地址续传、非分段服务器回退和逐字节一致性验证，以及上传分片重试、源文件变化检测、检查点恢复、旧数据库迁移、网盘识别、错误处理、下载域名边界和 Windows 文件名处理。真实百度上传、续传、下载和分享的验证结果见[验证记录](../VERIFICATION.md)；自动测试不会使用网盘账号。

## 目录

- `src/`：Vue 界面与样式。
- `src-tauri/src/providers/`：多网盘路由与百度接口。
- `src-tauri/src/quark.rs`：夸克接口。
- `src-tauri/src/session.rs`：独立登录窗口、后台会话读取。
- `src-tauri/src/migration.rs`：旧资料迁移。
- `src-tauri/src/lib.rs`：任务调度和 Tauri 命令。
- `src-tauri/src/engine.rs`：aria2 进程、RPC 和传输测试。
- `src/DownloadBox.vue`：统一下载输入、文件预览与 BT 参数界面。
- `src-tauri/src/bt.rs`、`download_commands.rs`：链接分类、种子校验、磁力元数据和 BT 任务。
- `src-tauri/src/engine_bt_tests.rs`：真实本地 Tracker／aria2 节点、选择下载及重启续传测试。
- `src-tauri/src/direct.rs`、`direct_commands.rs`：普通链接检测、文件名与续传保护、创建下载任务。
- `src-tauri/src/tray.rs`：托盘图标、恢复窗口和退出菜单。
- `src-tauri/src/store.rs`：SQLite 持久化。
- `src-tauri/src/providers/baidu_drive.rs`：百度个人网盘、上传与分享接口。
- `src-tauri/src/drive_commands.rs`：个人文件和上传任务命令。
- `src-tauri/src/upload.rs`：分片并发、重试、暂停续传和文件校验。
- `src-tauri/src/verification.rs`：仅开发构建启用的真实账号验证程序；只上传生成的测试文件。
- `third-party/notices/`：第三方许可资料。

aria2 的固定版本和校验值见 `third-party/notices/README.txt`。本项目未复制或运行 LinkSwift 脚本。

[返回项目首页](../README.md) · [使用指南](usage.md) · [验证记录](../VERIFICATION.md)
