# AgentGod Files 桌面版

两种应用，按需下载：

| 应用 | 用途 |
| --- | --- |
| **AgentGod Files Local** | 打开即可浏览本机文件；也可以添加并连接远程服务器 |
| **AgentGod Files Client** | 只作为远程客户端，支持保存和切换多台服务器 |

## 安装包

- **Windows**：下载 `*_x64-setup.exe`（推荐）或 `.msi`
- **macOS（Intel / Apple Silicon 通用）**：下载 `.dmg`
- **Linux**：下载 `.AppImage`（需 FUSE）或 `.deb`

## 说明

- 安装包未做商业代码签名。macOS 首次打开如被拦截，请右键「打开」，或执行
  `xattr -dr com.apple.quarantine "/Applications/AgentGod Files Local.app"`；
  Windows SmartScreen 请选择「更多信息 → 仍要运行」。
- 客户端连接自签名证书服务器时，请在服务器设置中勾选「允许自签名证书」。
- 从 0.1.1 起，预览内容同样使用 AES-256-GCM 加密；请将服务端与桌面端升级到同一版本。
- 桌面端会在本机回环地址启动一个仅本机可访问的伴生服务，用于加载界面和代理请求；
  不监听局域网，也不会上传任何数据到第三方。
- 服务端继续使用 Docker 部署；桌面端与其共用同一套加密登录与传输协议。

完整文档：https://github.com/fengqshanl/rs-file-broswer
