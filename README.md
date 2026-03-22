# discord-agent

DiscordからVPS上のCodex CLIを操作し、調査結果をNotionへ保存しつつ、Cloud Run上で公開RSSを配信するRust実装です。

## Binaries

- `bot`: Discordのスレッド投稿を受け取り、内部キューへ積み、Codexを実行し、結果をSQLiteとNotionへ保存します。
- `rss`: Cloud Run上で動作し、Notionの公開対象タスクを直接読み出して `/rss.xml` と `/tasks/:task_id` を公開します。

## Environment variables

### Shared

- `SQLITE_PATH`: SQLiteファイルパス。既定値は `data/discord-agent.sqlite3`
- `CODEX_BIN`: Codex CLIバイナリ名。既定値は `codex`
- `CODEX_MODEL`: 任意。Codex CLIへ `--model` で渡す
- `WORKER_CONCURRENCY`: ワーカー数。既定値は `1`
- `NOTION_TOKEN`: 任意。設定時のみNotion書き込み/読み出しを有効化
- `NOTION_TASK_DATABASE_ID`: 任意。Notion Task DBのID
- `PUBLIC_BASE_URL`: RSS serviceの公開ベースURL。既定値は `http://localhost:8080`
- `RSS_BIND_ADDR`: RSS serverのbind先。既定値は `0.0.0.0:8080`
- `PORT`: 任意。Cloud Runではこの値を優先して `0.0.0.0:$PORT` でlisten

### Discord bot

- `DISCORD_TOKEN`: 必須
- `DISCORD_ALLOWED_USER_IDS`: カンマ区切りのユーザーID allowlist
- `DISCORD_ALLOWED_ROLE_IDS`: カンマ区切りのロールID allowlist

## Notion database properties

Task DBには以下のプロパティ名を用意してください。

- `Task ID` (`rich_text`)
- `Title` (`title`)
- `Status` (`select`)
- `Task Type` (`select`)
- `Requester` (`rich_text`)
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
docker compose up -d
docker compose exec app cargo run --bin rss
```

## Current v1 behavior

- Discordのスレッド内投稿のみ処理します
- allowlist外ユーザーの実行は拒否します
- v1では research系タスクのみ実行し、coding系は将来の承認フロー用に予約されています
- Codex実行結果の要約をNotionへ保存し、`Publish=true` の完了タスクのみRSSに載せます

## Cloud Run

- `rss` バイナリは [Dockerfile](Dockerfile) でコンテナ化し、Cloud Runへデプロイします
- Cloud Run上の `rss` は GCS ではなく Notion API を直接参照します
- Cloud Runには `NOTION_TOKEN`, `NOTION_TASK_DATABASE_ID`, `PUBLIC_BASE_URL` を設定してください
- `PUBLIC_BASE_URL` は Cloud Runの公開URLを指定してください

## Documentation

- [Architecture](docs/architecture.md)
