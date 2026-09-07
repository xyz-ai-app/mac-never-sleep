# Self-host on Cloudflare

[简体中文](self-host.zh-CN.md)

Deploy the phone board, API, and Durable Objects to your own Cloudflare account. Macs connect directly to your deployment: pairing, status, and remote commands bypass the project's public service, moving their request and storage load to your account. Local Screen-Off Standby needs no server; remote access remains off by default.

## Deploy

You need a Cloudflare account, Node.js 22 or newer, and a clone of this repository. Run from the repository root:

```sh
npm install --no-package-lock
npx wrangler login
npx wrangler whoami
npx wrangler deploy --config wrangler.self-host.json --dry-run
npx wrangler deploy --config wrangler.self-host.json
```

Check the account with `whoami` first; set `CLOUDFLARE_ACCOUNT_ID` if you have multiple accounts. The separate `never-sleep-personal` Worker creates SQLite Durable Objects and uploads `site/`. No project gateway, D1, KV, or additional server is needed. Change `name` in the configuration first if your account already has a Worker with that name, to avoid overwriting it.

Copy the deployment's HTTPS URL, for example `https://never-sleep-personal.YOUR-SUBDOMAIN.workers.dev`. Open `/board/` (English) or `/zh/board/` (Chinese). Do not add the `/never-sleep` prefix. Always use `--config wrangler.self-host.json`: plain `npm run deploy` uses the project's public deployment configuration.

The configuration retains API entry rate limits and disables preview URLs and Workers observability. Pairing links use the request origin automatically. For a custom domain, follow the [Cloudflare configuration reference](https://developers.cloudflare.com/workers/wrangler/configuration/); use the same HTTPS domain on the Mac and phone. Do not put an interactive Access login in front of the API: the Mac client does not support Access authentication.

## Connect a Mac

1. Disable remote access in the existing app, then quit it from the menu. A running menu-bar process does not inherit environment changes from a new terminal.
2. Start with a separate data directory so public-service credentials are not reused. Adjust the application path if installed elsewhere:

```sh
NEVER_SLEEP_CLOUD_URL='https://never-sleep-personal.YOUR-SUBDOMAIN.workers.dev' \
NEVER_SLEEP_DATA_DIR="$HOME/Library/Application Support/Never Sleep Self Hosted" \
'/Applications/Never Sleep.app/Contents/MacOS/never-sleep' --menubar
```

3. Enable remote access in More Settings and click Pair. Verify that the QR code/pairing URL points to your domain, then open it on the phone. Alternatively, run this in another terminal:

```sh
NEVER_SLEEP_DATA_DIR="$HOME/Library/Application Support/Never Sleep Self Hosted" \
'/Applications/Never Sleep.app/Contents/MacOS/never-sleep' pair --json
```

Set the endpoint on the persistent application process. Setting it only on `pair` does not switch an existing instance. A separate data directory starts with separate settings and remote access disabled. Supply these variables on every launch. For a custom LaunchAgent, put both in `EnvironmentVariables` and disable the original app's login startup to avoid running two instances. A shell `export` alone does not configure apps launched from Finder.

## Verify and maintain

- Pairing URLs and the phone board's API/WebSocket requests must use your domain.
- Confirm the Mac is online; start/end standby from the phone. Disable remote access and confirm it appears offline within about 35 seconds.
- To update, pull a reviewed version, run `npm install --no-package-lock` and `npm test`, then deploy with the same configuration. Keep the Worker name and existing migration tags to preserve the storage namespace. Update the Worker before distributing a new Mac client.
- Usage and billing belong to your account. Monitor Workers and Durable Objects requests, duration, and storage in Cloudflare; this guide does not promise perpetual free hosting.
- To return to the public service, disable remote access, quit the self-hosted instance, and launch the original app normally. Pair separately with each deployment.

## Privacy boundary

This is hosting in your Cloudflare account, not offline operation or end-to-end encryption. The Worker processes device names, status, pairing credentials, and remote commands; Cloudflare still provides the network and storage. Phone credentials live in your domain's browser localStorage. Mac credentials live in `cloud.toml` in the chosen data directory. Keep these private.

Disabling remote access disconnects the client but does not delete existing server data. “Remove from this phone” only removes that browser's credentials. Neither action is a server-data deletion request. To decommission, disable remote access on all Macs, then clean up the Worker and its Durable Objects storage in Cloudflare and check for retained data or logs. There is currently no user-facing operation that wipes all server data.

The included marketing pages still link to the project website and downloads. These links do not carry pairing or device status, but following them visits public websites.
