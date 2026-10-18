# CloudDownloader 0.5.7

全新蓝白界面与深灰 Night 主题，优化下载工作区和八网盘文件浏览体验。

- 参考 Motrix 的视觉层次，重整浅蓝侧栏、白色工作区、蓝色按钮与进度条，以及字体、间距和圆角；应用、托盘和安装器图标同步为蓝色。
- Night 改为深灰表面与蓝色强调色，覆盖下载、网盘、上传、分享、账号及弹窗。主题切换仍位于最小化按钮左侧，并记住选择。
- 下载任务突出文件名、进度、实时速度和估算剩余时间，按文件类型区分图标；底部状态栏显示下载、上传速度与引擎状态。
- 下载列表独立滚动，网盘文件列表与底部选择操作区适配较小窗口；统一调整搜索、账号卡片、文件表格和删除确认弹窗。
- 沿用一个下载框自动识别 HTTP/HTTPS 文件、八网盘分享、磁力链接及种子，保留暂停、续传、BT 选文件和按需删除本地文件。

## 下载

- [Windows 安装版](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.7/CloudDownloader_0.5.7_x64-setup.exe)：适合日常使用，自动创建快捷方式。
- [Windows 便携版](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.7/CloudDownloader-0.5.7-win-x64.zip)：解压后运行根目录的 `CloudDownloader.exe`，保留完整目录结构。
- [SHA-256 校验文件](https://github.com/sclliang/CloudDownloader/releases/download/v0.5.7/SHA256SUMS-0.5.7.txt)。

需要 Windows 10/11 x64 和 Microsoft Edge WebView2 Runtime。安装版会在缺少运行时时引导下载。升级时保留原有登录资料与任务。

## 验证

完成八网盘、两套主题和四种窗口尺寸的 64 项布局检查，以及 8 组有任务页面检查、10 个 Night 场景、统一下载框五种入口和操作交互检查。最终发布目录已通过实际桌面启动与主题切换验证，主程序及安装器的版本属性和便携包校验通过。

本次主要更新界面与图标，下载与登录逻辑沿用 0.5.6。UC、蓝奏云及优享版的真实账号和传输验证范围沿用上一版，详见 [验证记录](https://github.com/sclliang/CloudDownloader/blob/v0.5.7/VERIFICATION.md)。
