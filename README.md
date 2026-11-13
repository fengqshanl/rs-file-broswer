# AgentGod Files

一个用 Rust 编写的轻量级自托管文件服务器：单二进制、低内存占用，面向低功耗设备（ARM64 单板机、NAS、旧笔记本）设计。自带加密登录、应用层加密传输、在线预览、带宽与并发保护，用 Docker 一条命令即可部署；同时提供 Windows 桌面应用（macOS / Linux 桌面端暂不发布）。

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Rust](https://img.shields.io/badge/rust-1.82%2B-orange)
![Platform](https://img.shields.io/badge/platform-windows%20%7C%20linux-lightgrey)

## 目录

- [特性](#特性)
- [界面预览](#界面预览)
- [桌面应用](#桌面应用windows)
- [快速开始](#快速开始)
- [配置](#配置)
- [使用说明](#使用说明)
- [安全设计](#安全设计)
- [资源保护](#资源保护)
- [项目结构](#项目结构)
- [协议摘要](#协议摘要)
- [测试](#测试)
- [部署建议](#部署建议)
- [常见问题](#常见问题)
- [已知限制](#已知限制)
- [安全边界](#安全边界)
- [许可证](#许可证)

## 特性

- **文件管理**：目录浏览、新建文件夹、重命名、递归删除
- **批量与文件夹上传**：多选文件、整目录上传、拖拽文件夹，自动创建子目录并保留空目录
- **文件与文件夹下载**：单文件流式下载；文件夹服务端流式打包为未压缩 `.tar`
- **在线预览**：图片 / SVG、视频、音频、PDF、HTML（沙箱静态）、文本 / 代码 / JSON、CSV / TSV、Word（`.docx`）、Excel（`.xlsx` / `.xlsm`）
- **全程加密传输**：上传、下载、文件夹打包与所有预览均使用 AES-256-GCM 应用层加密帧，拦截 HTTP 流量只能看到密文
- **边收边看**：文本按块解密即时追加；图片、PDF、Office 与音视频显示实时接收进度，音视频完整解密后播放
- **预览快捷键**：视频 `←`/`→` 快退/快进 5 秒、`空格` 播放/暂停；图片 `←`/`→` 在目录内循环切换
- **浏览历史**：自动记录访问过的文件夹与打开的文件，点击记录可跳回对应目录并高亮目标文件
- **大目录分页**：目录扫描一次并缓存，前端按页加载（滚动到底自动续载），加载期间显示骨架屏
- **账户管理**：登录后可修改用户名和密码，其他会话立即失效
- **加密登录**：RSA-OAEP 包装随机会话密钥，一次性 challenge + 强制时间戳防重放，Argon2id 校验密码
- **加密传输**：API、上传、下载与预览全程 AES-256-GCM，AEAD 附加数据绑定方法与 URI/流序号，防重放
- **HTTPS-only**：内置自签名证书生成，可替换为受信证书
- **资源保护**：全局令牌桶限速、传输/预览并发上限、磁盘安全余量、容器资源限制
- **单文件部署**：前端资源编译进二进制，运行时镜像约 15 MB，空闲内存约 1–3 MB
- **桌面多标签工作区**：本机、AgentGod 服务器与 SSH 主机各自一个标签，可同时登录、独立浏览；标签间可复制、剪切、粘贴文件与文件夹，传输全程经加密流中转；支持原生窗口与系统文件拖拽
- **SSH / SFTP 目标**：在桌面端直接管理远程服务器文件，支持密码与私钥认证；凭据存入系统凭据库，主机密钥首次确认后固定，变化即拒绝

## 界面预览

> 截图使用演示数据。

![文件列表](docs/ui-files.jpg)

![在线预览](docs/ui-preview.jpg)

## 桌面应用（Windows）

从 [GitHub Releases](https://github.com/fengqshanl/rs-file-broswer/releases) 下载安装包，无需 Docker、Node.js 或浏览器。

> 当前只发布 Windows x64 MSI 安装包。macOS 与 Linux 桌面端暂停发布，历史 Release 中的对应安装包已移除；服务端仍可通过 Docker 或二进制在 Linux / Windows 上运行。

**AgentGod Files** 将原来的 Local / Client 两个应用合并为一个：启动即浏览本机文件，右上角「连接管理」可以添加远程目标，每个目标生成独立标签。

| 目标类型 | 认证方式 | 能力 |
| --- | --- | --- |
| **本机** | 无需登录（仅本机回环服务） | 浏览、上传、下载、预览、重命名、删除 |
| **AgentGod Files 服务器** | 用户名 + 密码（加密会话） | 与上面相同，支持 HTTPS / HTTP 与自签名证书 |
| **SSH / SFTP** | 密码或私钥 | 与上面相同，直接管理远程服务器文件 |

安装包：

- **Windows**：`*_x64.msi`，64 位 Windows 10 / 11（ARM64 系统通过系统模拟运行）
- **应用内热更新包**：`*.msi.sig` 与 `latest.json`（配置签名密钥后由发布工作流生成）

SSH 凭据只写入系统凭据库（Windows 凭据管理器 / macOS 钥匙串），配置文件 `targets.json` 不保存密码或私钥口令。首次连接主机会显示 SHA256 指纹，确认后记住；指纹变化会拒绝连接并提示，防止中间人攻击。

桌面端采用「本机优先 + 多标签」工作区：启动后先显示本机文件，右上角「连接管理」支持添加 AgentGod 服务器（HTTPS / HTTP）或 SSH / SFTP 主机。每个标签有独立的会话、路径、筛选和分页状态，切换标签不会打断另一个标签的浏览。

标签间操作与本地文件管理器一致：

- 在任意文件的复制 / 剪切按钮（或 `Cmd/Ctrl+C`、`Cmd/Ctrl+X`）后切换到目标标签，点击「粘贴」或按 `Cmd/Ctrl+V`
- 支持文件、空文件夹和递归文件夹；同名目标自动生成「(副本)」「(2)」等名称
- 跨标签复制时，数据先按源端密钥解密，再按目标端密钥重新加密上传；源端和目标端之间的网络传输只包含密文
- 桌面端复用与服务端相同的界面、加密登录和传输协议。伴生服务只监听本机回环地址，局域网不可访问

桌面端的远程连接面向同版本 AgentGod Files 服务端（HTTPS / 明文 HTTP，HTTP 仅建议用于回环或可信内网），并内置 SSH / SFTP 目标；WebDAV、SMB 等通用文件协议尚未支持。

安装包未做商业代码签名：首次运行时 Windows SmartScreen 可能提示「Windows 已保护你的电脑」，点「更多信息 → 仍要运行」即可。

### 应用内更新与热更新

**0.1.3 起**，桌面应用启动后会在后台检查 GitHub Releases（不阻塞界面、离线时静默跳过，结果缓存 30 分钟）。发现新版本会弹出提示，并按当前平台与架构匹配 Windows MSI 安装包：

- **应用内更新**：Release 附带已签名的 MSI 更新包与清单时，直接在应用内下载、校验签名、安装，完成后点「重启应用」生效
- **下载安装包**：始终可用，由系统默认浏览器打开已匹配好的 `.msi`；没有匹配包时打开发布页
- **跳过此版本**：记录在 `localStorage`，下次启动不再提示同一版本；右上角「检查更新」可随时强制重查
- **非 Windows 平台**：仍可正常检查更新，但会明确提示「当前仅发布 Windows 安装包」，不再给出无法下载的链接

发布工作流在配置签名密钥后，会把 `*.msi.sig` 与 `latest.json` 一起挂到 Release；未配置密钥时只发布 `.msi`，用户可手动下载升级。

#### 更新源无法访问时（镜像 / 代理）

应用默认直连 `api.github.com` 与 `github.com`。在无法访问 GitHub 的网络里，「检查更新」会失败；此时可在系统数据目录放一个 `update-source.json`（macOS 为 `~/Library/Application Support/AgentGodFiles/`，Windows 为 `%APPDATA%\AgentGodFiles\`），把检查与下载都指向自建镜像：

```json
{
  "api_url": "https://mirror.example.com/api/latest-release",
  "download_prefix": "https://mirror.example.com/releases/latest/download/",
  "release_page": "https://mirror.example.com/releases"
}
```

- `api_url` 需要返回与 GitHub `releases/latest` 相同结构的 JSON（`tag_name`、`body`、`assets[]`）
- `download_prefix` 用来拼出 `latest.json`；清单里的签名包 URL 指向镜像即可
- 可选 `updater_endpoints`：直接指定签名清单地址数组，覆盖默认拼接（适用于清单与实际文件不在同一前缀的镜像）
- 也可用环境变量覆盖：`AGENTGOD_UPDATE_API`、`AGENTGOD_UPDATE_DOWNLOAD_PREFIX`、`AGENTGOD_UPDATE_RELEASE_PAGE`、`AGENTGOD_UPDATER_ENDPOINTS`（逗号分隔）
- 除 `http://127.0.0.1` / `http://localhost` 外只接受 `https://`；配置无效时自动回退到 GitHub，不影响其他功能

镜像站只需原样代理 GitHub 的 Release 资产（`.msi`、`.msi.sig`、`latest.json`）。签名仍在本地校验，镜像无法注入内容。检查失败时，界面提示会带上该配置文件的路径。

> **从 0.1.6 升级**：0.2.0 起产品改名为 **AgentGod Files**，安装包标识与 UpgradeCode 均已变化，无法覆盖安装或应用内更新。请先卸载旧版 Local / Client，再安装 0.2.0 一次；旧版的 `servers-local.json` / `servers-client.json` 会在首次启动时自动合并为 `targets.json`。

**维护者：启用静默热更新**（Tauri updater 强制要求签名，无法关闭）：

1. 生成密钥对：`cargo tauri signer generate -w ~/.tauri/agentgod-files.key`（如设置了密码，请妥善保存）
2. 公钥内容写入 `desktop/app/tauri.conf.json` 的 `plugins.updater.pubkey`（仓库中已填好本次生成的公钥）
3. 在 GitHub 仓库 **Settings → Secrets and variables → Actions** 添加：
   - `TAURI_SIGNING_PRIVATE_KEY`：`~/.tauri/agentgod-files.key` 的完整内容（base64 单行）。也接受解码后的两行文本，工作流会统一规范化
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：生成密钥时设置的密码；未设置密码则留空
4. 推送 `desktop-v*` 标签：工作流在 `windows-latest` 上执行 `tauri build --bundles msi`，设置 `bundle.createUpdaterArtifacts = true`，产出 `.msi` / `.msi.sig`，再生成 `latest.json` 挂到同一个 Release。**不要把 `updater` 写进 `--bundles`**：它不是合法的 bundle target，会让构建直接失败

发布流程在构建前会先用私钥签名一个测试文件，并用 `scripts/verify-updater-signature.mjs` 校验签名与仓库中的 `pubkey` 一致；构建后再对每个 `.app.tar.gz` 做同样校验。这样能避免“构建成功、用户下载后却因签名不匹配而安装失败”的情况。常见错误与处理：

- **`Invalid symbol 10 / 32 / 37`**：粘贴内容多了换行、粘贴成了解码后的两行文本，或末尾带了 `%` 等 URL 编码字符。`scripts/normalize-updater-key.mjs` 会自动识别并还原成标准 `.key` 内容，无需手工处理；完全无法识别时会在预检步骤直接说明拿到的是公钥、签名文件还是无效内容
- **`Wrong password for that key`**：`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 与生成密钥时不一致，或密钥有密码但未配置该 secret
- **签名 ID 与 pubkey 不匹配**：私钥不是生成当前 `pubkey` 的那一把。若旧私钥已丢失，只能更换 `pubkey` 并发布新版，同时让已安装用户手动下载一次

预检失败时，Actions 的错误摘要会写明签名材料类型（私钥 / 公钥 / 签名文件）、是否配置了密码，以及签名器返回的错误行。

未配置签名 secret 时发布流程照常进行，只是不含静默更新包：用户仍会收到新版提示，并能一键下载 Windows 安装包。**私钥丢失后无法再为已安装用户签发更新，请离线备份，绝不要提交到仓库。**

### 从源码构建桌面端

```bash
# 需要 Rust 与 Tauri 2 的系统依赖，见 https://v2.tauri.app/start/prerequisites/
cargo install tauri-cli --version "^2"

cd desktop/app && cargo tauri build
```

安装包输出在 `desktop/app/target/release/bundle/msi`。推送 `desktop-v*` 标签会触发 [桌面发布工作流](.github/workflows/desktop-release.yml)，自动构建 Windows `.msi` 与签名更新包并上传到 Release；macOS / Linux 安装包暂不构建。

桌面端配置保存在系统数据目录（Windows 为 `%APPDATA%\AgentGodFiles\`，macOS 为 `~/Library/Application Support/AgentGodFiles/`）下的 `targets.json`；SSH 密码与私钥口令单独存入系统凭据库，`known_hosts.json` 保存已确认的主机指纹。

## 快速开始

### 环境要求

- Docker 24+ 与 Docker Compose v2
- 一个用于存放文件的宿主机目录

### Docker Compose（推荐）

```bash
git clone https://github.com/fengqshanl/rs-file-broswer.git
cd rs-file-broswer

cp .env.example .env
# 至少修改 ADMIN_PASSWORD、FILES_DIR、LISTEN_IP
chmod 600 .env

docker compose up -d --build
docker compose ps
docker compose logs -f
```

启动后访问 `https://<LISTEN_IP>:8090/`。默认只绑定 `127.0.0.1`，如需局域网访问，把 `.env` 中的 `LISTEN_IP` 改为服务器内网地址（不要直接暴露公网）。

首次启动会自动生成：

| 文件 | 用途 |
| --- | --- |
| `state/rsa_private.pem` | 登录用 RSA 私钥（0600） |
| `state/tls/cert.pem` / `state/tls/key.pem` | 自签名 TLS 证书与私钥 |
| `state/tls/self-signed.json` | 自签名证书到期时间（用于自动轮换，0600） |
| `state/credentials.json` | 管理员凭据哈希（0600，首次登录前创建） |

自签名证书首次访问会提示不安全，选择「高级 → 继续前往」即可。浏览器的 WebCrypto 要求安全上下文，因此独立服务端只提供 HTTPS。自签名证书有效期 365 天，到期前 30 天会在启动时自动重新签发（浏览器需重新信任一次）；运维自行放置的受信证书不会被覆盖。仅在回环代理或可信内网调试时可用 `TLS_DISABLED=1` 关闭 TLS，但此时浏览器 WebCrypto 不可用。

### Docker 单容器

```bash
docker build -t agentgod-files .

docker run -d --name agentgod-files \
  --restart unless-stopped \
  --read-only --tmpfs /tmp:size=16m \
  --cap-drop ALL --cap-add DAC_OVERRIDE --cap-add DAC_READ_SEARCH \
  --security-opt no-new-privileges:true \
  --memory 256m --cpus 1.0 --pids-limit 128 \
  -p 127.0.0.1:8090:8090 \
  -v /path/to/files:/data/files \
  -v "$PWD/state:/data/state" \
  -e ADMIN_PASSWORD='change-me-to-a-long-random-password' \
  -e TLS_EXTRA_IPS=127.0.0.1 \
  agentgod-files
```

> 容器以 root 运行，因为需要读写宿主机上 root 拥有的目录。请配合上面的 capability 裁剪、只读根文件系统与资源限制使用。

### 从源码运行

需要 Rust 1.82+：

```bash
export DATA_ROOT=/path/to/files
export TMP_ROOT=/path/to/files/.agentgod-tmp
export STATE_DIR=$PWD/state
export ADMIN_USER=admin
export ADMIN_PASSWORD='change-me-to-a-long-random-password'
export BIND=127.0.0.1
export PORT=8090
cargo run --release
```

## 配置

所有配置通过环境变量提供。Docker Compose 会从 `.env` 读取同名变量；其中 `LISTEN_IP`、`HOST_PORT`、`FILES_DIR`、`TZ` 只由 Compose 使用（端口映射、目录挂载与容器时区），其余会原样传给容器内的服务进程。

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `ADMIN_USER` | `admin` | 初始管理员用户名（仅首次初始化凭据时使用） |
| `ADMIN_PASSWORD` | 必填 | 初始密码，至少 8 个字符 |
| `ADMIN_PASSWORD_HASH` | 空 | 可选，Argon2id PHC 字符串；设置后优先于明文密码 |
| `LISTEN_IP` | `127.0.0.1` | 宿主机监听地址；局域网访问改为内网 IP 或 `0.0.0.0` |
| `HOST_PORT` | `8090` | 宿主机端口 |
| `FILES_DIR` | `./data` | 对外提供的宿主机目录（挂载到容器 `/data/files`） |
| `TLS_EXTRA_IPS` | `127.0.0.1` | 证书 SAN 中额外包含的 IP，逗号分隔 |
| `TLS_EXTRA_DNS` | `localhost` | 证书 SAN 中额外包含的域名 |
| `UPLOAD_RATE_MBPS` | `20` | 全局上传限速（MB/s），`0` 为不限速 |
| `DOWNLOAD_RATE_MBPS` | `30` | 全局下载与预览限速（MB/s），`0` 为不限速 |
| `MAX_CONCURRENT_UPLOADS` | `2` | 并发上传数（1–16） |
| `MAX_CONCURRENT_DOWNLOADS` | `2` | 并发下载数（1–16） |
| `MAX_CONCURRENT_PREVIEWS` | `4` | 并发预览请求数（1–32） |
| `MAX_ACTIVE_UPLOADS` | `32` | 同时存在的未完成上传会话上限 |
| `MAX_FILE_SIZE_GB` | `100` | 单文件大小上限（GB） |
| `DISK_RESERVE_MB` | `2048` | 磁盘保留空间（MB），上传不会挤占该余量 |
| `SESSION_TTL_SECS` | `7200` | 会话滑动有效期（秒） |
| `SESSION_MAX_TTL_SECS` | `86400` | 会话绝对有效期上限（秒） |
| `RSA_BITS` | `2048` | 首次生成 RSA 私钥长度（2048/3072/4096） |
| `LOGIN_MAX_FAILURES` | `8` | 登录失败次数上限 |
| `LOGIN_WINDOW_SECS` | `600` | 登录失败计数与封禁窗口（秒） |
| `TZ` | `Asia/Shanghai` | 容器时区 |

### 源码直跑 / 自建镜像的环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `DATA_ROOT` | `/data/files` | 对外提供的数据目录；Compose 中由 `FILES_DIR` 挂载到该路径 |
| `TMP_ROOT` | `DATA_ROOT/.agentgod-tmp` | 上传临时目录，**必须是 `DATA_ROOT` 内的独立子目录**，否则启动报错（保证上传完成后可原子重命名） |
| `STATE_DIR` | `/data/state` | RSA 私钥、凭据哈希与 TLS 目录；Compose 中挂载 `./state` |
| `TLS_DIR` | `STATE_DIR/tls` | 证书、私钥与自签名元数据目录 |
| `BIND` | `0.0.0.0` | 容器内监听地址；宿主机绑定由 `LISTEN_IP` 控制 |
| `PORT` | `8090` | 监听端口 |
| `LOCAL_MODE` | `0` | 桌面本机模式：免登录、忽略 Bearer、仅接受回环 Host。由桌面伴生服务设置，**不要在独立服务端开启** |
| `TLS_DISABLED` | `0` | 关闭 TLS 并改用明文 HTTP；仅限回环代理或可信内网调试，浏览器 WebCrypto 此时不可用 |
| `ADMIN_USER` / `ADMIN_PASSWORD` / `ADMIN_PASSWORD_HASH` | 同上表 | 凭据初始化 |
| 限速、并发、会话、登录防护等变量 | 同上表 | 同名直接传入容器 |

构建参数（可选，用于选择基础镜像）：

```bash
docker build \
  --build-arg RUST_IMAGE=rust:1-alpine \
  --build-arg ALPINE_IMAGE=alpine:3.20 \
  -t agentgod-files .
```

在中国大陆网络下可以改用镜像源，例如 `docker.1ms.run/library/rust:1-alpine`。

### 使用受信证书

把证书与私钥放到 `state/tls/cert.pem` 和 `state/tls/key.pem`，然后重启容器。证书需覆盖你实际访问的域名或 IP，此后浏览器不再提示自签名警告。没有 `self-signed.json` 元数据且证书主题不是本项目生成的文件会被原样保留，自动轮换不会覆盖运维提供的证书。

### 重置管理员凭据

如果忘记密码，删除 `state/credentials.json` 并重启容器，会重新使用 `.env` 中的 `ADMIN_USER` / `ADMIN_PASSWORD` 初始化：

```bash
rm state/credentials.json
docker compose up -d --force-recreate
```

## 使用说明

- **上传**：点击「上传」多选文件；点击「文件夹」选择整个目录；也可以把文件或文件夹直接拖进页面
- **下载**：右键文件或文件夹选择「下载」/「下载文件夹 (.tar)」；文件夹会打包为 `<名称>.tar` 下载
- **预览**：点击文件名。图片、视频、音频、PDF、HTML、文本、Office 文档均可在线查看
- **右键菜单**：文件与文件夹的操作（打开、下载、复制、剪切、重命名、删除）都在右键菜单中；列表不再占用一列显示操作按钮
- **多选与快捷键**：`Ctrl/Cmd+单击` 多选、`Shift+单击` 连续选择；`Ctrl/Cmd+C` / `X` / `V` 复制、剪切、粘贴，`Ctrl/Cmd+A` 全选，`F2` 重命名，`Delete` 删除，`Enter` 打开，`F5` 刷新
- **预览快捷键**：预览打开时，视频 `←`/`→` 快退/快进 5 秒、`空格` 播放/暂停；图片 `←`/`→` 循环切换目录内图片
- **隐藏文件**：默认不显示以 `.` 开头或带 Windows 隐藏属性的项目；勾选工具栏「显示隐藏文件」后显示，选择会记住
- **文件夹大小**：文件列表会异步计算并显示文件夹的递归大小（跳过符号链接），超大目录显示 `≥` 下限；结果按目录缓存
- **桌面标签**：桌面端启动后默认显示本机文件；顶部命令栏的「添加连接」可添加 AgentGod 服务器或 SSH 主机，每台服务器生成独立标签
- **跨标签复制**：右键菜单或 `Ctrl/Cmd+C`、`Ctrl/Cmd+X` 复制/剪切（支持多选）；切换到目标标签后点「粘贴」或按 `Ctrl/Cmd+V`。文件夹会递归复制，同名项目自动改名
- **账户**：点击右上角「账户」或左上角用户名，可修改用户名与密码
- **历史**：点击工具栏时钟图标，可查看最近访问的文件夹与文件；点击记录回到对应目录并高亮该文件，可一键清空
- **大目录**：首次打开会显示骨架屏，列表滚动到底自动加载下一页；底部也会显示「加载更多」按钮
- **筛选**：工具栏搜索框按名称过滤**已加载**的项目；大目录可继续滚动加载后再筛选

## 安全设计

### 传输与登录

1. **TLS 1.2 / 1.3**：服务只监听 HTTPS，浏览器必须处于安全上下文才能使用 WebCrypto。
2. **登录（非对称 + 一次性）**：浏览器先取 `GET /api/challenge` 拿到 120 秒有效的一次性随机值，再拉取 RSA 公钥、生成随机 AES-256 会话密钥并用 `RSA-OAEP-SHA256` 包装；用户名、密码、客户端时间戳与 challenge 以 AES-256-GCM 加密后发送。服务器强制校验时间戳（±10 分钟）、在密码校验前原子消费 challenge，然后用 Argon2id 校验密码并签发随机会话令牌；截获的登录密文第二次提交会被拒绝。
3. **数据（对称 + 防重放）**：所有 API 请求/响应 JSON 使用 AES-256-GCM，AAD 绑定 `方法 + URI`，密文不能跨接口重放；服务端另外按会话记录最近 8,192 个请求 nonce（覆盖会话绝对有效期），同一密文第二次送达直接以 409 拒绝。客户端重试必须重新生成信封，不得重发原密文。
4. **上传**：按 2 MiB 分块加密，服务器校验分块序号、声明大小与 GCM 标签，完成后在同一文件系统内原子重命名。
5. **下载与预览**：64 KiB 流式加密帧 `[u32 长度][12B nonce][密文+tag]`；下载每帧 AAD 含序号，预览每帧 AAD 绑定随机流 ID 与序号；文件夹归档额外带结束标记，客户端校验完整性。
6. **密码存储**：Argon2id（默认参数），也可直接提供 `ADMIN_PASSWORD_HASH`。
7. **会话**：32 字节随机令牌，放在 `Authorization: Bearer` 头中（不使用 Cookie，天然规避 CSRF）；滑动过期 + 绝对上限；刷新页面会丢失内存中的会话密钥，需要重新登录。
8. **爆破防护**：按来源 IP 限制登录失败次数，Argon2 校验并发限制为 2，避免登录请求打满 CPU。
9. **路径安全**：拒绝 `..`，对目标做 canonicalize 并校验位于数据根目录内；临时上传目录对外不可见；删除符号链接只删除链接本身。
10. **浏览器侧**：严格 CSP（无内联脚本/样式）、`X-Frame-Options: DENY`、`nosniff`、无 Referrer、同源限制；文件名通过 `textContent` 渲染，避免 XSS。

### 文件预览的安全模型

预览不再暴露任何明文文件地址，`/preview/<token>` 已移除：

- `POST /api/preview/stream` 使用正常的 Bearer 会话与加密请求 `{path}`，响应是 AES-256-GCM 帧流；每帧 AAD 绑定随机流 ID 与序号，响应头只包含大小、MIME 和流 ID，不包含文件内容。
- 前端通过 `fetch()` 接收密文并在内存逐帧解密。文本边解密边追加；图片、PDF、HTML、Office 与音视频在完整接收后创建短期 `blob:` URL 呈现，切换或关闭预览时立即吊销。原生 `<img>` / `<video>` / `<iframe>` 从不接触明文 HTTP 流。
- 预览仍受全局下载限速与预览并发限制。为避免浏览器内存耗尽，加密预览有大小上限：图片 128 MB、PDF 256 MB、音视频 512 MB、HTML 32 MB；超过上限请下载后查看。
- HTML 预览运行在 `iframe sandbox` 的独立不透明源中（没有 `allow-same-origin`），并注入禁止脚本、外部网络和表单提交的 CSP，因此只做静态渲染，无法读取本应用 DOM、令牌或调用已认证接口。SVG 通过 `<img>` 加载，脚本不会执行。
- Word / Excel 完全在浏览器本地解析（仓库内打包的 mammoth.js 与 SheetJS，按需加载），文件不会发送给第三方；docx 生成的 HTML 在插入前经过白名单清洗。

> 应用层加密保护的是「浏览器 ↔ 本服务」这条会话。自签名证书需要手动信任；若用户在已被 MITM 的环境里强行信任伪造证书，应用层密钥交换无法阻止。需要严格防护时请使用受信证书。

### 桌面端安全模型

- 本机版与客户端都会在 `127.0.0.1` 的随机端口启动一个前端伴生服务，只监听回环地址；Tauri WebView 只加载这个地址，界面与 API 同源，无需放宽 CORS。
- 远程请求由伴生服务按标签转发，只透传必要请求头（`Authorization`、`Content-Type` 等），响应体按流转发；预览帧在浏览器端解密，桌面代理转发时同样只有密文。默认严格校验服务器证书，只有显式勾选「允许自签名证书」才关闭校验。
- 本机标签不做登录，因为访问者必须是同一台机器上的本地进程；切换根目录时会 canonicalize 并校验目录存在，文件操作仍限制在当前根目录内。远程标签沿用完整的 RSA-OAEP + AES-GCM 登录流程，每个标签独立保存内存会话。
- 跨标签复制不会在磁盘上产生明文临时文件：源端加密帧在页面内存中解密后立即按目标端会话密钥重新分块加密，再写入目标端。
- 伴生服务不监听局域网、不启用 CORS 通配，也不接入任何遥测或第三方服务。
- 更新检查只读取公开的 GitHub Releases API，不上传机器标识、文件路径或凭据；请求 8 秒超时、失败静默，Webview 仍保持 `connect-src 'self'`。静默更新包安装前必须由 Tauri 用内置公钥校验签名，校验不通过不会安装；下载链接由伴生服务从已校验的 Release 资产里选取，前端无法传入任意 URL。

### 资源保护

- **全局令牌桶限速**：上传、下载（含预览）各自一个全局桶，所有连接共享，而不是每连接限速。
- **并发上限**：默认 2 个上传 + 2 个下载，预览另设 4 个并发；其余请求排队。
- **内存可控**：下载块 64 KiB、上传块最大 4 MiB，流式处理，不缓存整个文件；预览同样流式，单次读取 64 KiB。
- **磁盘安全余量**：拒绝任何会占用「剩余空间 − 2 GiB」的上传，分块写入时持续检查。
- **大目录保护**：目录在阻塞线程中一次扫描并排序，结果缓存 15 秒（最多缓存 2 个目录），前端每页 300 项；单目录上限 50,000 项。这样翻页不会反复遍历目录，也不会把数万个 DOM 节点一次性塞进浏览器。
- **容器限制**：示例配置使用 256 MB 内存、1 CPU、128 PID、只读根文件系统、丢弃全部 capability（仅保留文件访问所需的 `DAC_OVERRIDE` / `DAC_READ_SEARCH`），日志轮转 10 MB × 3。
- **登录 CPU 保护**：Argon2 并发信号量 + 失败退避。

## 项目结构

```
src/
  main.rs        二进制入口
  lib.rs         库入口（桌面端复用同一套服务与界面）
  server.rs      HTTP / HTTPS 启动与优雅关闭
  config.rs      环境变量解析与校验
  app.rs         桌面本机模式信息与根目录切换
  credentials.rs 管理员凭据持久化
  auth.rs        登录、会话提取器、登录限流、账户修改
  crypto.rs      RSA-OAEP / AES-256-GCM / Argon2 / AAD
  api.rs         加密请求读取与加密响应
  files.rs       浏览、批量/文件夹上传、下载、重命名、删除
  archive.rs     流式 tar 打包（目录下载）
  preview.rs     加密预览流、MIME 推断
  rate.rs        全局令牌桶
  state.rs       应用状态、上传会话、会话表、预览并发控制
  tls.rs         自签名证书生成/复用
  web.rs         路由、CSP、安全响应头、静态资源
static/          嵌入式 Web UI（原生 JS + WebCrypto，无外部依赖）
static/vendor/   Word/Excel 本地解析库（仅预览时按需加载）
desktop/common/  共享伴生运行时：回环前端、本机服务、HTTP 代理、SSH/SFTP 目标
  src/config.rs    targets.json（HTTP + SSH 目标）与旧配置迁移
  src/known_hosts.rs  SSH 主机指纹（TOFU）持久化
  src/secrets.rs   系统凭据库中的 SSH 密码 / 私钥口令
  src/ssh/         SSH 连接、SFTP 文件系统、流式 tar 与逐目标回环服务
  src/update.rs    版本检查、Windows 安装包匹配、静默更新状态桥接
desktop/app/     AgentGod Files Tauri 2 应用（本机 + 服务器 + SSH）
desktop/web/     打包用占位页面（真实界面由伴生服务提供）
scripts/         Node E2E 协议测试、Chrome UI 冒烟测试、测试视频生成
  build-updater-manifest.mjs  生成单应用的签名更新清单
  normalize-updater-key.mjs   把签名私钥规范化为 tauri-cli 要求的 .key 格式
  verify-updater-signature.mjs 校验更新包签名与应用内嵌 pubkey 是否匹配
.github/         GitHub Actions 桌面发布工作流与 Release 说明
Dockerfile       Alpine + musl 构建
docker-compose.yml
```

## 协议摘要

| 步骤 | 内容 |
| --- | --- |
| `GET /api/challenge` | 返回 120 秒有效的一次性登录 challenge 与 `expires_in`（`no-store`） |
| `GET /api/pubkey` | 返回 RSA 公钥 PEM |
| `POST /api/login` | `{ek, iv, ct}`：RSA-OAEP 包装的会话密钥 + AES-GCM 加密的 `{username,password,ts,challenge}`；`ts` 必填，challenge 在校验前被消费 |
| 登录响应 | AES-GCM 加密的 `{token,expires_in,user}`，AAD = `login-resp` |
| 其他 API | `Authorization: Bearer <token>` + `{iv,ct}`，AAD = `<METHOD> <URI>` |
| `POST /api/list` | 加密 `{path, offset, limit}` → 当前页目录项、`total`、`has_more`、剩余空间（扫描结果服务端缓存 15 秒） |
| `POST /api/mkdir` / `ensure_dirs` | 创建单个/批量文件夹 |
| `POST /api/rename` / `delete` | 重命名、递归删除 |
| `POST /api/upload/init` | 加密 `{dir,name,size}` → `{id}` |
| `PUT /api/upload/chunk/{id}/{seq}` | 原始加密帧 `[u32 ct_len][12B iv][ct]`，响应加密 JSON |
| `POST /api/upload/finish` | 校验大小并原子移动；`/abort` 清理临时文件 |
| `POST /api/download` | 加密 `{path}`；响应为带 `x-file-size`/`x-chunk-size` 的加密帧流 |
| `POST /api/download_folder` | 加密 `{path}`；tar 加密帧流，最后一帧为 `AGF_TAR_END_v1` |
| `POST /api/preview/stream` | 加密 `{path}`；响应为 `x-file-size` / `x-chunk-size` / `x-stream-id` / `x-preview-content-type` 的 AES-GCM 帧流 |
| `POST /api/account/update` | 加密 `{current_password,new_username,new_password}`，改密后吊销其他会话 |
| 下载帧 | `[u32 ct_len][12B iv][ct]`，AAD = `dl:<seq>` |
| 预览帧 | `[u32 ct_len][12B iv][ct]`，AAD = `pv:<stream-id>:<seq>` |

## 测试

```bash
# Rust 单元测试
cargo test
cargo test --manifest-path desktop/common/Cargo.toml   # 目标模型、凭据、SSH 与更新匹配

# 更新清单生成逻辑自测（无需联网）
node scripts/build-updater-manifest.mjs --self-test

# 桌面端共享运行层（本机 + HTTP + SSH 目标）
cargo run --release --manifest-path desktop/common/Cargo.toml --bin dev_app

# Tauri 应用
cd desktop/app && cargo tauri dev
```

`dev_app` 会打印一个 `http://127.0.0.1:<端口>/` 地址，可直接用 Chrome 验证桌面端界面与代理行为，无需启动 Tauri 窗口。测试时可用 `AGENTGOD_DATA_DIR` 指定独立数据目录、`AGENTGOD_LOCAL_ROOT` 指定本机标签展示的目录、`AGENTGOD_MEMORY_SECRETS=1` 用内存凭据库代替系统凭据库。

本地启动服务（用于下面的 E2E 与 UI 测试）：

```bash
mkdir -p testdata/files testdata/state

DATA_ROOT=$PWD/testdata/files \
TMP_ROOT=$PWD/testdata/files/.agentgod-tmp \
STATE_DIR=$PWD/testdata/state \
TLS_DIR=$PWD/testdata/state/tls \
ADMIN_USER=admin \
ADMIN_PASSWORD='local-test-password-123' \
BIND=127.0.0.1 PORT=18443 TLS_EXTRA_IPS=127.0.0.1 \
UPLOAD_RATE_MBPS=0 DOWNLOAD_RATE_MBPS=0 \
cargo run
```

端到端协议测试（登录失败、一次性 challenge 与时间戳校验、登录密文与 API 密文重放拒绝、加密 API、路径穿越、分块上传密文、嵌套目录、流式下载哈希、文件夹 tar、加密预览帧与明文泄漏检查、账户修改与他人会话吊销等）：

```bash
BASE_URL=https://127.0.0.1:18443 \
ADMIN_PASSWORD='local-test-password-123' \
NODE_TLS_REJECT_UNAUTHORIZED=0 \
node scripts/e2e-test.mjs
```

浏览器 UI 冒烟测试（需要本地 Chrome 与调试端口；视频快捷键测试需要 ffmpeg 生成 6 秒测试片段）：

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --remote-debugging-port=9223 \
  --user-data-dir=/tmp/agentgod-chrome --ignore-certificate-errors about:blank &

./scripts/make-test-video.sh /tmp/agentgod-ui-test.webm

CHROME_DEBUG_URL=http://127.0.0.1:9223 \
UI_URL=https://127.0.0.1:18443/ \
ADMIN_PASSWORD='local-test-password-123' \
node scripts/ui-smoke.mjs
```

桌面多标签回归测试（需要全新的 `dev_app` 数据目录、Chrome 调试端口和上一节的远程测试服务器）：

```bash
rm -rf /tmp/agentgod-tabs-home /tmp/agentgod-tabs-data
mkdir -p /tmp/agentgod-tabs-home

AGENTGOD_DATA_DIR=/tmp/agentgod-tabs-data \
AGENTGOD_LOCAL_ROOT=/tmp/agentgod-tabs-home \
AGENTGOD_MEMORY_SECRETS=1 \
  ./desktop/common/target/release/dev_app
# 记下命令输出的 http://127.0.0.1:<端口>/ 地址

FRONT_URL=http://127.0.0.1:<端口>/ \
REMOTE_URL=https://127.0.0.1:18443 \
REMOTE_USER=admin REMOTE_PASS='local-test-password-123' \
LOCAL_DIR=/tmp/agentgod-tabs-home \
CHROME_DEBUG_URL=http://127.0.0.1:9223 \
node scripts/ui-tabs.mjs
```

该测试覆盖本机优先启动、连接管理面板不会卡死、服务器标签与加密登录、远程预览、跨标签文件/文件夹复制、剪切移动、移除服务器标签和刷新后的本地自动登录。顶部命令栏、隐藏文件开关、文件夹大小、右键菜单、多选与剪贴板快捷键由 `scripts/ui-features-smoke.mjs` 覆盖（使用下方 fixture）：

```bash
FRONT_URL=http://127.0.0.1:<端口>/ \
CHROME_DEBUG_URL=http://127.0.0.1:9223 \
node scripts/ui-features-smoke.mjs
```

SSH 目标的表单、标签、连接报错、编辑与删除由 `scripts/ssh-ui-smoke.mjs` 覆盖：

```bash
FRONT_URL=http://127.0.0.1:<端口>/ \
CHROME_DEBUG_URL=http://127.0.0.1:9223 \
node scripts/ssh-ui-smoke.mjs
```

SSH / SFTP 协议链路（TOFU、加密登录、列表、分页、分块上传、下载、预览、文件夹 tar、重命名、删除、指纹持久化与错误密码）由内嵌的测试 SSH 服务器覆盖，不需要真实主机；同一套测试还会走一遍伴生层的添加、信任指纹、测试连接、修改凭据、清除指纹与删除目标：

```bash
cargo test --release --manifest-path desktop/common/Cargo.toml --test ssh_e2e --test ssh_companion
```

## 部署建议

- **只绑定内网地址**：默认 `127.0.0.1`。需要远程访问时绑定 Tailscale / WireGuard / 局域网 IP，或放在已有 VPN 之后。
- **不要直接暴露公网**：本项目的定位是私有文件入口，没有多用户隔离、审计与防病毒能力。
- **使用受信证书**：如果通过域名访问，建议放置受信证书或由反向代理终止 TLS（反向代理需支持大文件与流式响应透传）。
- **备份**：文件在 `FILES_DIR`；`state/` 保存 RSA 私钥、TLS 证书与凭据哈希，建议一起备份。
- **升级**：替换镜像后 `docker compose up -d`。上传过程中重启会让未完成上传失效，临时文件会在下次启动时清理。
- **观察**：`docker stats agentgod-files` 通常只有数 MB 内存；若持续升高，检查限速与并发配置。

## 常见问题

**用 `http://` 访问报 `curl: (1) Received HTTP/0.9` 或浏览器直接报错？**

独立服务端默认只提供 HTTPS。请使用 `https://`，自签名证书需要加 `-k`（curl）或手动信任（浏览器）。仅在显式设置 `TLS_DISABLED=1` 时才会启用明文 HTTP，且此时浏览器 WebCrypto 不可用。

**页面登录后刷新就需要重新登录？**

会话密钥只保存在页面内存中，这是有意设计。刷新会丢弃密钥。

**浏览器提示「当前页面不是安全上下文」？**

说明你用的是 `http://` 或证书未被信任，WebCrypto 因此不可用。请改用 HTTPS 并信任证书。

**大文件下载失败，提示超过 1 GB？**

不支持 File System Access 的浏览器会回退为内存 Blob，上限 1 GiB。建议使用 Chrome / Edge。

**视频无法播放？**

浏览器不支持该容器或编码（例如 H.265/HEVC、部分 MKV）。可下载后用本地播放器打开；mp4（H.264）与 webm（VP8/VP9）兼容性最好。

**`.doc` / `.xls` / `.pptx` 无法预览？**

当前只支持 `.docx` 与 `.xlsx` / `.xlsm` 的本地解析，旧版二进制格式与演示文稿请下载查看。

**修改密码后其他设备被登出？**

这是预期行为：修改账户会吊销其他会话，当前会话保持登录。进行中的预览会随请求结束而失效。

**为什么视频/PDF 预览要等一会儿才能播放，且不能像以前一样边下边播？**

因为预览内容也走 AES-256-GCM 应用层加密，浏览器原生媒体元素无法直接解密分块响应。文本仍支持边解密边查看；图片、PDF、Office 与音视频会显示接收进度，音视频完整解密后即可正常播放和拖动。超过预览大小上限的文件请下载后查看。

**升级到 0.1.2 需要注意什么？**

0.1.2 起登录必须先调用 `GET /api/challenge`，并把一次性 challenge 放进加密登录载荷；时间戳也变为必填。**这是一次不兼容的协议变更**：0.1.1 及更早的桌面客户端无法登录 0.1.2 服务端，0.1.2 桌面客户端也无法登录旧版服务端，请同步升级服务端与桌面端；浏览器端强制刷新页面即可（静态资源为 `no-cache`）。此外会话内重复的加密请求会被拒绝，自定义客户端重试时须重新生成信封。旧版本生成的自签名证书会在升级后首次启动时重新签发一次，浏览器需要重新信任。

**升级到 0.1.1 需要注意什么？**

0.1.1 起预览内容也使用 AES-256-GCM 应用层加密，旧的明文预览接口已移除。服务端与桌面端必须一起升级：0.1.0 桌面端无法预览 0.1.1 服务端，0.1.1 桌面端也无法使用 0.1.0 服务端的预览功能。

**桌面版和浏览器版有什么区别？**

浏览器版需要先部署服务器，再通过 HTTPS 打开页面。桌面版分成两个应用，二者都默认显示本机文件，也都可以添加远程服务器：本机版更偏向单机文件管理，客户端同样支持本机文件与多服务器标签工作区，只是使用独立的服务器配置。桌面版复用同一套加密协议和界面，安装后无需浏览器。

**桌面客户端支持哪些服务器协议？**

当前支持同版本 AgentGod Files 服务端的 HTTPS / HTTP 连接，每台服务器一个标签；HTTP 只建议用于本机回环或可信内网。SFTP、WebDAV、SMB、FTP 等通用协议尚未内置，不在当前版本的能力范围内。

**Windows 提示「Windows 已保护你的电脑」或 SmartScreen 拦截？**

安装包未做商业代码签名。点「更多信息 → 仍要运行」即可；也可在文件属性中勾选「解除锁定」。企业环境可用组策略或 `Unblock-File` 批量解除。

## 已知限制

- 单管理员、单租户，没有多用户与权限分组
- 不支持断点续传
- 不支持在线预览旧版 Office（`.doc`/`.xls`）与 `.pptx`
- HTML 预览在沙箱中静态渲染，脚本、外部 CSS/JS/图片与表单提交均被禁用；复杂页面可能只显示基本结构
- 文件夹下载为未压缩 tar（降低 CPU 占用），下载后需自行解压
- 表格预览最多 2000 行 × 80 列；Word/Excel 预览上限 64 MB（需完整接收后解析，期间显示进度）；文本流式显示上限 32 MB
- 加密预览需要先完整接收并解密：图片 128 MB、PDF 256 MB、音视频 512 MB、HTML 32 MB；音视频完整接收后才能开始播放和任意拖动
- 单目录最多显示 50,000 项；筛选只作用于已加载的分页内容
- 浏览历史保存在浏览器 `localStorage`（最多 300 条），不跨浏览器同步
- 文件在宿主机磁盘上不额外加密存储，访问控制依赖容器与宿主机文件权限
- 符号链接按保守策略处理：列表会解析指向并拒绝越出数据根目录的目标，删除只删除链接本身，文件夹打包跳过全部符号链接，避免把链接目标带出根目录
- 自签名证书有效期 365 天、到期前 30 天自动轮换；运维自行放置的受信证书不会被覆盖
- 桌面客户端目前只支持连接同版本 AgentGod Files 服务端（HTTPS / HTTP），不支持 SFTP、WebDAV、SMB、FTP
- 桌面安装包未做商业代码签名，首次安装需要手动放行
- 0.1.2 及更早的桌面安装包没有内置更新检查，需要手动升级一次到 0.1.3 或更新版本；未配置 `TAURI_SIGNING_PRIVATE_KEY` 的发布不含静默更新包，只能下载安装包手动升级
- 桌面本机标签按「同一台机器的本地访问即可信」设计，不设登录；多人共用电脑时请使用操作系统账户隔离

## 安全边界

本项目按「带认证的私有文件管理入口」设计，请只绑定内网或 VPN 地址，并妥善保管密码。它不提供多租户隔离、持久化审计、病毒扫描或文件类型限制，不建议直接暴露到公网。发现安全问题请通过仓库 issue 私下联系维护者，不要公开披露细节。

## 许可证

[MIT](LICENSE)。第三方前端库许可见 [`static/vendor/NOTICE.md`](static/vendor/NOTICE.md)。

## 贡献

欢迎提交 Issue 与 Pull Request。提交前请确保：

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
