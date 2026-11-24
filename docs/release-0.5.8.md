## 123 云盘下载

- 修复个人网盘和外部分享下载时无法识别 `123295.com` 下载节点的问题，同时支持 `share.123pan.cn` 下的分享链接。
- 下载节点连接或 TLS 握手短暂失败时，程序会重新获取下载地址并尝试单连接下载，减少任务停在 0% 并报“错误 1”的情况。
- 在 Windows 本机分别从个人网盘和外部分享完整下载了文件。两份文件的大小和 MD5 均与网盘记录一致。

## 批量任务管理

- 支持一键暂停或继续全部任务，也可以多选任务后暂停、继续或移除。批量移除前会请求确认，并保留已下载的文件。感谢 sandrone01 贡献批量操作。[PR #1](https://github.com/sclliang/CloudDownloader/pull/1)
- 修复重启后继续下载和移除失败时的任务状态处理。执行批量暂停时，正在解析的磁力链接可以继续读取元数据。

## Windows x64 下载

- 运行安装包可覆盖升级现有版本。[下载安装包](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.8/CloudDownloader_0.5.8_x64-setup.exe)
- 使用便携版时，解压整个 ZIP 后运行 `CloudDownloader.exe`。[下载便携版](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.8/CloudDownloader-0.5.8-win-x64.zip)
- 可用 SHA-256 核对以上两个文件。[下载校验文件](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.8/SHA256SUMS-0.5.8.txt)
