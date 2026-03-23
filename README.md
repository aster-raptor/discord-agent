# discord-agent

DiscordからVPS上のCodex CLIを操作し、調査結果をNotionへ保存しつつ、Vercel上のNext.jsアプリから公開RSSを配信する実装です。

## Quick Start

1. GitHub Releases の最新バージョンを開き、`discord-agent-bot-linux-x86_64-<version>.tar.gz` をダウンロードします。

```bash
curl -LO https://github.com/aster-raptor/discord-agent/releases/download/v0.1.0/discord-agent-bot-linux-x86_64-v0.1.0.tar.gz
tar -xzf discord-agent-bot-linux-x86_64-v0.1.0.tar.gz
cd discord-agent-bot-linux-x86_64-v0.1.0
```

最新版を使うときは、URL とディレクトリ名の `v0.1.0` を対象バージョンに読み替えてください。

2. Bot に必要な設定を用意します。

- Discord Bot と Notion の初期設定は [Notion and Discord Setup](docs/notion-discord-setup.md) を参照してください
- 最低限 `DISCORD_TOKEN` を設定してください
- Notion 連携と公開 URL 連携を使う場合は `NOTION_TOKEN`, `NOTION_TASK_DATABASE_ID`, `PUBLIC_BASE_URL` も設定してください

3. 環境変数を設定して Bot を起動します。

```bash
export DISCORD_TOKEN=your_discord_token
export NOTION_TOKEN=your_notion_token
export NOTION_TASK_DATABASE_ID=your_notion_database_id
export PUBLIC_BASE_URL=https://your-public-app.example.com
./bot
```

Linux 上でデーモンとして動かす場合は systemd service を作成します。

`/etc/systemd/system/discord-agent-bot.service`:

```ini
[Unit]
Description=discord-agent bot
After=network.target

[Service]
Type=simple
User=your_user
WorkingDirectory=/opt/discord-agent-bot-linux-x86_64-v0.1.0
Environment=DISCORD_TOKEN=your_discord_token
Environment=NOTION_TOKEN=your_notion_token
Environment=NOTION_TASK_DATABASE_ID=your_notion_database_id
Environment=PUBLIC_BASE_URL=https://your-public-app.example.com
ExecStart=/opt/discord-agent-bot-linux-x86_64-v0.1.0/bot
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

有効化と起動:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now discord-agent-bot
sudo systemctl status discord-agent-bot
```

停止と再起動:

```bash
sudo systemctl stop discord-agent-bot
sudo systemctl restart discord-agent-bot
```

4. 公開 RSS を使う場合は Vercel 側も設定します。

- 手順は [Vercel Setup](docs/vercel-setup.md) を参照してください
- Bot 側の `PUBLIC_BASE_URL` は Vercel の公開 URL と同じ値にしてください

## Discord からの依頼手順

1. Bot を追加した Discord サーバー内でスレッドを開きます。
2. そのスレッドに、調査したい内容をそのままメッセージで投稿します。
3. URL を含めると、Bot は本文に加えて `Referenced URLs` として URL 一覧も Codex に渡します。
4. 受理されると、Discord 上で `Accepted task ... Status: queued` と `Task ID` が返ります。
5. その後、同じスレッドに `running`、`summarizing`、`completed` または `failed` の進捗が返ります。

依頼メッセージ例:

```text
このページの内容を調べて、要点を日本語で要約してください。
https://example.com/article
```

注意:

- DM は受け付けず、Discord サーバー内のスレッド投稿だけを処理します
- v1 は research タスク専用です
- `codex exec`、`コードを書いて`、`fix`、`refactor` を含む依頼は coding タスクとして扱われ、現在は実行されません

## Binaries

- `bot`: Discordのスレッド投稿を受け取り、内部キューへ積み、Codexを実行し、結果をSQLiteとNotionへ保存します。
- `public app`: Vercel上で動作する Next.js/TypeScript アプリです。Notionの公開対象タスクを直接読み出して `/rss.xml` と `/tasks/:task_id` を公開します。

## Environment variables

### Shared

- `SQLITE_PATH`: SQLiteファイルパス。既定値は `data/discord-agent.sqlite3`
- `CODEX_BIN`: Codex CLIバイナリ名。既定値は `codex`
- `CODEX_MODEL`: 任意。Codex CLIへ `--model` で渡す
- `WORKER_CONCURRENCY`: ワーカー数。既定値は `1`
- `NOTION_TOKEN`: 任意。設定時のみNotion書き込み/読み出しを有効化
- `NOTION_TASK_DATABASE_ID`: 任意。Notion Task DBのID
- `PUBLIC_BASE_URL`: 公開RSSアプリの公開ベースURL。既定値は `http://localhost:3000`

### Discord bot

- `DISCORD_TOKEN`: 必須

## Notion database properties

Task DBには以下のプロパティ名を用意してください。

- `Task ID` (`rich_text`)
- `Title` (`title`)
- `Status` (`select`)
- `Task Type` (`select`)
- `Publish` (`checkbox`)
- `Public Summary` (`rich_text`)
- `Updated At` (`date`)
- `Completed At` (`date`)
- `Thread ID` (`rich_text`)
- `Public URL` (`rich_text`)

## Run

```bash
docker compose run --rm app cargo test
```

```bash
docker compose build
docker compose run --rm app npm install
docker compose run --rm app npm test
docker compose run --rm app npm run build
docker compose up -d
docker compose exec app npm run dev
```

## Current v1 behavior

- Discordのスレッド内投稿のみ処理します
- 一人利用前提のため、Discord内の追加認可制御は行いません
- v1では research系タスクのみ実行し、coding系は将来の承認フロー用に予約されています
- Codex実行結果の要約をNotionへ保存し、`Publish=true` の完了タスクのみRSSに載せます

## Vercel

- 公開面は Next.js の Route Handlers で実装しています
- Vercelには `NOTION_TOKEN`, `NOTION_TASK_DATABASE_ID`, `PUBLIC_BASE_URL` を設定してください
- `PUBLIC_BASE_URL` は Vercel の公開URLを指定してください
- ローカル検証は [Dockerfile.app](Dockerfile.app) と `docker compose` を使います

## GitHub Actions

- `.github/workflows/ci-bot.yml` は `main` への push で Rust の `bot` バイナリを Linux x86_64 向けにビルドし、Actions artifact `bot-linux-x86_64` を作ります
- `.github/workflows/release-bot.yml` は `v*` タグの push だけで動き、GitHub Release に `discord-agent-bot-linux-x86_64-<version>.tar.gz` を添付します
- ダウンロード時は Release asset を展開し、中の `bot` 実行ファイルを使ってください
- CI 上の Rust ビルドは GitHub Actions runner で直接実行します。ローカル開発用の Docker Compose 方針とは用途が異なります

## Documentation

- [Architecture](docs/architecture.md)
- [Notion and Discord Setup](docs/notion-discord-setup.md)
- [Vercel Setup](docs/vercel-setup.md)

