## 123 云盘下载

- 兼容新版下载接口及多节点地址响应，校验候选节点并依次尝试；保留旧接口兼容路径。
- 修正新版分享接口的文件 ID 类型和字段名称，明确提示登录过期与免费流量不足。

## 夸克下载与批量准备

- 个人网盘和外部分享统一使用下载凭据，同一批任务复用有效凭据，过期后刷新。
- 网络短暂中断时进行有限重试；下载节点连接失败时重新获取地址并尝试单连接下载。
- 同一批任务复用会话与目录信息，最多同时准备 3 个任务，减少重复请求；批量创建任务记录使用事务，保持文件和任务对应关系。

## 验证范围

- 77 项 Rust 自动化测试通过，1 项需单独指定在线分享的测试跳过。
- 123 个人网盘文件完整下载，大小和 MD5 与网盘记录一致。
- 夸克个人网盘和外部分享各完整下载 2 个文件，大小与网盘记录一致；接口未提供可用于比对的 MD5。
- **123 外部分享本次返回官方错误码 5113（本月免费流量不足），未完成完整下载验证。**

## Windows x64 下载

- [下载安装包](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.9/CloudDownloader_0.5.9_x64-setup.exe)，可覆盖升级现有版本。
- [下载便携 ZIP](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.9/CloudDownloader-0.5.9-win-x64.zip)，解压整个目录后运行 `CloudDownloader.exe`。
- [下载 SHA-256 校验文件](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.9/SHA256SUMS-0.5.9.txt)。
