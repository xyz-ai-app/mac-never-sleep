# Cloudflare 自部署

[English](self-host.md)

把手机看板、API 和 Durable Objects 部署到自己的 Cloudflare 账户，Mac 直接连接自己的地址，配对、状态和远程指令不经过项目公共服务。这也将对应的请求和存储负载移出公共服务。仅本地关屏待命无需部署，远程访问默认关闭。

## 部署

需要 Cloudflare 账户、Node.js 22 或更新版本，以及本仓库的克隆。以下命令在仓库根目录运行：

```sh
npm install --no-package-lock
npx wrangler login
npx wrangler whoami
npx wrangler deploy --config wrangler.self-host.json --dry-run
npx wrangler deploy --config wrangler.self-host.json
```

先用 `whoami` 确认目标账户。多账户用户可以设置 `CLOUDFLARE_ACCOUNT_ID`。部署使用独立的 `never-sleep-personal` Worker，自动建立 SQLite Durable Objects，并上传 `site/`。无需公共网关、D1、KV 或额外服务器。若同一账户已有同名 Worker，先修改配置中的 `name`，避免覆盖。

复制部署输出的 HTTPS 地址，例如 `https://never-sleep-personal.YOUR-SUBDOMAIN.workers.dev`。英文看板在 `/board/`，中文看板在 `/zh/board/`。不要加 `/never-sleep` 前缀。普通 `npm run deploy` 使用项目公共部署配置，自部署必须始终带上 `--config wrangler.self-host.json`。

配置保留 API 入口限流，关闭预览 URL 和 Workers observability。默认从请求地址生成配对链接。自定义域名可按 [Cloudflare 配置文档](https://developers.cloudflare.com/workers/wrangler/configuration/)设置；Mac 和手机必须使用同一个 HTTPS 域名。不要在 API 前添加需要交互登录的 Access 页面，当前 Mac 客户端没有 Access 登录支持。

## 连接 Mac

1. 在现有应用里关闭远程访问，然后从菜单退出应用。已经运行的菜单栏进程不会读取新终端的环境变量。
2. 用独立数据目录启动，避免复用公共服务的设备凭据。以下假设应用安装在 `/Applications`；按实际安装路径调整：

```sh
NEVER_SLEEP_CLOUD_URL='https://never-sleep-personal.YOUR-SUBDOMAIN.workers.dev' \
NEVER_SLEEP_DATA_DIR="$HOME/Library/Application Support/Never Sleep Self Hosted" \
'/Applications/Never Sleep.app/Contents/MacOS/never-sleep' --menubar
```

3. 在新实例的更多设置中开启远程访问，点击配对。确认二维码/配对 URL 指向自己的域名，在手机打开它完成配对。也可以在另一个终端执行：

```sh
NEVER_SLEEP_DATA_DIR="$HOME/Library/Application Support/Never Sleep Self Hosted" \
'/Applications/Never Sleep.app/Contents/MacOS/never-sleep' pair --json
```

环境变量必须设置在持续运行的应用进程上，只给 `pair` 命令设置服务器地址不会切换已有实例。独立数据目录有独立设置，首次启动远程访问仍关闭。每次启动都需要上述环境变量；若自行创建 LaunchAgent，将两个变量放到 `EnvironmentVariables`，并关闭原应用的开机启动，避免两个实例同时运行。仅在 shell 配置里 `export` 不会改变 Finder 启动的应用。

## 验收与维护

- 配对 URL 应使用自己的域名；手机看板的 `/api/*` 和 WebSocket 请求也应指向该域名。
- 确认 Mac 在线，从手机开始/结束待命，再关闭 Mac 远程访问，确认看板约 35 秒内显示离线。
- 更新时拉取经过审查的版本，运行 `npm install --no-package-lock` 和 `npm test`，用同一配置再次部署。保留 Worker 名称和已有 migration 标签，避免意外创建新存储。先更新 Worker，再更新 Mac 客户端。
- 账户用量和费用由用户承担，请在 Cloudflare 控制台查看 Workers、Durable Objects 请求、执行时间和存储；不承诺永久免费。
- 恢复公共服务：关闭远程访问、退出自部署实例，再正常启动原应用。自部署实例和公共服务需要分别配对。

## 隐私边界

这是自己的 Cloudflare 账户托管，不是离线或端到端加密。Worker 会处理设备名称、设备状态、配对凭据和远程命令，Cloudflare 仍承担网络和存储服务。手机凭据保存在该域名的浏览器 localStorage，Mac 凭据保存在所选数据目录的 `cloud.toml`；不要分享这些凭据。

关闭远程访问会断开连接，但不会删除服务端已有数据；手机的“从本机列表移除”只移除该浏览器的凭据。不要把退出或移除设备当作数据删除。彻底停用时先关闭所有 Mac 的远程访问，再在 Cloudflare 中清理对应 Worker 及 Durable Objects 存储，并检查是否仍有保留的数据或日志。当前没有一键清空全部服务端数据的用户界面。

随站点提供的介绍页仍含项目官网和下载链接；这些链接不承载配对或设备状态，但主动打开会访问公共网站。
