# CloudDownloader 0.4.2 验证记录

验证日期：2026-10-18。环境：Windows x64、Edge/WebView2。
0.4.1 及更早版本的验证记录保存在 `docs/verification-*.md`。

## 本次修改

- 移除游戏主题文案、角色立绘、切换组件及其随包资源。
- 下载、个人网盘、上传、分享与弹窗统一使用淡紫色主题，移除立绘后内容使用完整宽度。
- 应用图标改为紫色，主窗口使用浅色标题栏，托盘提示统一为通用下载器名称。
- 发布目录改为 `package/CloudDownloader.exe` 和 `package/bin/aria2c.exe`，按 EXE 目录定位下载引擎，同时兼容原安装布局。
- 补全 Windows 英文功能说明、产品名称、公司、作者版权、四段版本、内部名称、原始文件名和英语（美国）版本资源。

## 自动检查

- TypeScript 类型检查、Vite 前端构建、Prettier 检查、Rust 格式检查和 Git 差异空白检查通过。
- Rust 完整回归：21 项通过、0 项失败，1 项依赖显式提供真实分享链接的测试按设计忽略。
- 非分段下载回归曾在启用全局限速后等待进度超时。该用例改用独立引擎，避免前一个已完成的 24 MiB 下载影响新的限速统计，并将进度等待上限从 10 秒延长至 30 秒；保留原有 512 KiB 进度门槛、暂停检查点、不使用分段续传和最终逐字节一致性检查。下载协议实现保持原有行为。
- aria2 SHA-256 与随附许可记录一致：`BE2099C214F63A3CB4954B09A0BECD6E2E34660B886D4C898D260FEBFE9D70C2`。

## 界面预览

- 已检查下载工作台、普通链接表单与账号管理页面，淡紫配色和完整内容宽度正常，无角色立绘或切换按钮。
- 浏览器预览仅用于查看样式；下载引擎与 Tauri 命令以正式 EXE 的实际启动验证为准。

## 发布包验证

- Tauri 0.4.2 正式发布构建通过；主程序为 `package/CloudDownloader.exe`，下载引擎与第三方许可放在 `package/bin/`。
- 主 EXE 版本属性核对通过：CompanyName 为 `sixunited`，FileDescription 为 `Cloud file and transfer manager for Windows`，ProductName 与 InternalName 为 `CloudDownloader`，OriginalFilename 为 `CloudDownloader.exe`，FileVersion 和 ProductVersion 的字符串与数字版本均为 `0.4.2.0`，版权为 `Copyright (c) 2026 sixunited. Author: sclliang`，语言为英语（美国）。
- PE 依赖检查：主程序与 aria2 仅静态导入 Windows 系统 DLL，无需随包分发其他 DLL。系统仍需已安装 WebView2 Runtime。
- 清空开发工具 PATH（仅保留 Windows 和 System32），从项目的 `.verification/` 工作目录直接启动 `package/CloudDownloader.exe` 成功。正式窗口显示 `v0.4.2`、淡紫色主题和「下载引擎已就绪」，无预览环境中的 Tauri 调用错误。
- 实际子进程路径为 `package/bin/aria2c.exe`，验证主程序按 EXE 位置寻找引擎，不依赖启动目录、SDK 或开发环境 PATH。
- 最终 Rust 全量串行回归：21 项通过，0 项失败，1 项按设计忽略。未使用真实网盘账号执行传输。
