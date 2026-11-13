# AgentGod Files 桌面版

**AgentGod Files** 是合并后的单一桌面应用：默认展示本机文件系统；右上角「连接管理」可添加 AgentGod Files 服务器（HTTPS / HTTP）或 SSH / SFTP 主机，每个目标生成独立标签。

> **发布范围**：当前只提供 Windows x64 `.msi` 安装包。macOS 与 Linux 桌面端暂停发布，历史 Release 中的对应安装包已移除；服务端仍支持 Linux / Windows 部署。

## 0.2.0 更新

- 合并原 Local / Client 两个应用为 **AgentGod Files**，只发布一个安装包
- 新增 **SSH / SFTP** 目标：浏览、上传、下载、预览、新建文件夹、重命名、删除与文件夹 tar 下载
- SSH 支持密码与私钥；密码和私钥口令只写入 Windows 凭据管理器 / macOS 钥匙串，`targets.json` 不保存明文凭据
- SSH 主机密钥采用 TOFU：首次连接展示 SHA256 指纹待确认，记住后指纹变化会拒绝连接；可在「连接管理」中测试连接或清除旧指纹
- 首次启动自动把旧 `servers-local.json` / `servers-client.json` 合并为 `targets.json`（按 URL 去重，原文件保留备份）
- 更新清单统一为 `latest.json`，安装包名为 `AgentGod.Files_0.2.0_x64.msi`
- **升级说明**：应用标识与 UpgradeCode 已变化，0.1.6 的 Local / Client 用户需手动卸载旧版后安装 0.2.0；此后可正常应用内更新

## 0.1.6 更新

- 发布渠道改为 **Windows x64 MSI**：每次发版产出 `AgentGod.Files.Local_*_x64.msi` 与 `AgentGod.Files.Client_*_x64.msi`
- 应用内热更新在 Windows 上可用：Release 附带 `*.msi.sig`，`latest-local.json` / `latest-client.json` 提供 `windows-x86_64` 与 `windows-aarch64` 两个平台条目（ARM64 系统通过模拟运行 x64 包）
- 修复 Web 与桌面端上传：文件/文件夹选择框不再因 `display:none` 被 Safari / WKWebView 拦截，单文件与整目录上传均可正常触发
- 修复桌面端下载：WKWebView 默认会取消下载，现由应用显式接受并保存到系统「下载」目录（同名自动去重）；并为旧版 WebKit 增加无流式响应体时的回退
- 桌面端新增可配置更新源（`update-source.json` / `AGENTGOD_UPDATE_*` 环境变量）：GitHub 不可达时可指向镜像，检查与签名更新都走同一地址；检查失败会直接提示配置文件位置
- 服务端无需同步升级；旧版 macOS / Windows 用户需手动下载一次本版本

## 0.1.5 更新

- 修复应用内热更新的发布链路：签名私钥改为按 tauri-cli 要求的 `.key` base64 格式传入，不再把内容解码后写入（后者会报 `failed to decode base64 secret key`）
- 发布前先用私钥签名测试文件，并校验签名与两个应用内嵌的 `pubkey` 一致；构建后再校验每个 `.app.tar.gz`，签名不匹配会立即失败
- 兼容 tauri-action 带空格的更新包文件名，`latest-local.json` / `latest-client.json` 可正确生成
- 已安装 0.1.3 / 0.1.4 的 macOS 用户会收到 0.1.5 更新提示，可直接应用内更新并重启
- Release 资产继续保持 macOS-only，并自动清理历史 Windows / Linux 安装包

## 0.1.4 更新

- 发布范围收敛为 macOS：Release 只提供通用 `.dmg`，不再发布 Windows / Linux 安装包
- 修正签名更新包生成方式：由 `bundle.createUpdaterArtifacts` + `TAURI_SIGNING_PRIVATE_KEY` 产出 `.app.tar.gz` / `.sig`，不再把 `updater` 传给 `--bundles`（那会让构建失败）
- 更新清单 `latest-local.json` / `latest-client.json` 只包含 `darwin-x86_64` / `darwin-aarch64`；非 macOS 平台会明确提示暂无对应安装包
- 发布流程新增签名私钥格式预检与签名资产可见性等待：配置了密钥却没有产出签名包时直接失败，不再静默降级
- 功能与 0.1.3 一致，服务端无需同步升级

> 0.1.2 及更早版本仍需手动下载一次 macOS 安装包；0.1.3 及以后的 macOS 版本可自动接收更新提示。

## 0.1.3 更新

- 桌面端启动后自动检查新版本，并按当前系统、CPU 架构与应用类型（Local / Client）匹配对应安装包，弹窗显示更新说明
- 新增「应用内更新」：下载并校验签名更新包后在应用内安装，重启即生效（发布时需配置更新签名密钥）
- 新增「下载安装包」「跳过此版本」与右上角手动「检查更新」；离线或 GitHub 不可达时静默跳过，不影响使用
- **升级提示**：0.1.2 及更早版本不含更新检查逻辑，请手动下载一次 0.1.3；此后每次发版都会自动提醒并可应用内更新

## 0.1.2 更新

- 登录改用一次性 challenge + 强制时间戳校验，并拒绝会话内重复的加密请求，防止截获密文后重放
- 拖拽超大目录树时改为小批量创建目录，失败会明确提示，不再静默丢失空目录
- 自签名 TLS 证书改为 365 天有效期、到期前自动轮换，且不会覆盖运维自行放置的受信证书
- 桌面伴生运行时改由 Tauri 托管生命周期，移除堆上的永久泄漏对象
- **不兼容变更**：0.1.2 桌面端只能连接 0.1.2 服务端，请同步升级两端

## 0.1.1 更新

- 客户端与本机版改为「本机优先 + 多标签」工作区，启动不再进入空白连接页
- 右上角「添加服务器」使用 WebView 安全的自定义面板，修复部分系统上点击后卡死的问题
- 每台服务器独立标签、独立登录会话、独立路径与分页状态
- 标签间支持复制、剪切、粘贴文件与文件夹，同名自动改名，跨端传输只在内存中解密并立即重新加密
- 继续使用 AES-256-GCM 加密预览、下载、上传与文件夹归档

## 安装包

- **Windows x64**：下载 `AgentGod.Files_*_x64.msi`
- **应用内更新**：配置签名密钥后，Release 还会附带 `*.msi.sig` 与 `latest.json`
- **macOS / Linux**：暂停发布，历史安装包已移除

## 说明

- 安装包未做商业代码签名。Windows SmartScreen 首次运行可能拦截，点「更多信息 → 仍要运行」，
  或在下载目录执行 `Get-ChildItem *.msi | Unblock-File`。
- 连接自签名证书的 AgentGod 服务器时，请在连接设置中勾选「允许自签名证书」。
- 服务器目标面向同版本 AgentGod Files 服务端的 HTTPS / HTTP；SSH / SFTP 目标直接连接 OpenSSH 等标准服务器。
- 从 0.1.1 起，预览内容同样使用 AES-256-GCM 加密；请将服务端与桌面端升级到同一版本。
- 桌面端会在本机回环地址启动一个仅本机可访问的伴生服务，用于加载界面、代理请求和承载 SSH 服务；
  不监听局域网，也不会上传任何数据到第三方。0.1.3 起会读取公开的 GitHub Releases 元数据检查新版本，
  不发送任何本机文件或凭据；离线时自动跳过。
- 服务端继续使用 Docker 部署；桌面端与其共用同一套加密登录与传输协议。

完整文档：https://github.com/fengqshanl/rs-file-broswer
