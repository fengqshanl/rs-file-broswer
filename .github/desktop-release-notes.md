# AgentGod Files 桌面版

两种应用，按需下载：

| 应用 | 用途 |
| --- | --- |
| **AgentGod Files Local** | 打开即可浏览本机文件；右上角「添加服务器」连接多台远程服务器 |
| **AgentGod Files Client** | 同样默认显示本机文件，支持添加多台服务器并在同一窗口内用标签切换 |

## 0.1.1 更新

- 客户端与本机版改为「本机优先 + 多标签」工作区，启动不再进入空白连接页
- 右上角「添加服务器」使用 WebView 安全的自定义面板，修复部分系统上点击后卡死的问题
- 每台服务器独立标签、独立登录会话、独立路径与分页状态
- 标签间支持复制、剪切、粘贴文件与文件夹，同名自动改名，跨端传输只在内存中解密并立即重新加密
- 继续使用 AES-256-GCM 加密预览、下载、上传与文件夹归档

## 安装包

- **Windows**：下载 `*_x64-setup.exe`（推荐）或 `.msi`
- **macOS（Intel / Apple Silicon 通用）**：下载 `.dmg`
- **Linux**：下载 `.AppImage`（需 FUSE）或 `.deb`

## 说明

- 安装包未做商业代码签名。macOS 首次打开如被拦截，请右键「打开」，或执行
  `xattr -dr com.apple.quarantine "/Applications/AgentGod Files Local.app"`；
  Windows SmartScreen 请选择「更多信息 → 仍要运行」。
- 客户端连接自签名证书服务器时，请在服务器设置中勾选「允许自签名证书」。
- 服务器标签当前支持同版本 AgentGod Files 服务端的 HTTPS / HTTP；SFTP、WebDAV、SMB 等协议尚未内置。
- 从 0.1.1 起，预览内容同样使用 AES-256-GCM 加密；请将服务端与桌面端升级到同一版本。
- 桌面端会在本机回环地址启动一个仅本机可访问的伴生服务，用于加载界面和代理请求；
  不监听局域网，也不会上传任何数据到第三方。
- 服务端继续使用 Docker 部署；桌面端与其共用同一套加密登录与传输协议。

完整文档：https://github.com/fengqshanl/rs-file-broswer
