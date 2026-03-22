# Notion と Discord の設定手順

## 概要

このドキュメントは、`discord-agent` の Bot を動かすために必要な Discord と Notion の初期設定をまとめたものです。対象は Rust 製の `bot` で、公開面の Vercel 設定は `docs/vercel-setup.md` を参照してください。

このセットアップで行うこと:

- Discord Bot を作成する
- Bot をサーバーへ招待する
- 実行を許可するユーザーまたはロールを決める
- Notion integration を作成する
- Task Database を作成して integration に共有する
- Bot の環境変数を設定する

## 1. Discord 設定

### 1-1. Discord アプリケーション作成

1. Discord Developer Portal を開く
2. `New Application` を作成する
3. 任意のアプリ名を設定する
4. `Bot` セクションで Bot を追加する
5. `Reset Token` または `Copy Token` で Bot token を取得する

取得した token は `DISCORD_TOKEN` に設定します。

```text
DISCORD_TOKEN=xxxxxxxxxxxxxxxx
```

### 1-2. Privileged Gateway Intents

この Bot は以下の intents を使います。

- `GUILD_MESSAGES`
- `MESSAGE_CONTENT`
- `GUILD_MEMBERS`

そのため Discord Developer Portal の Bot 設定で少なくとも以下を有効化してください。

- `MESSAGE CONTENT INTENT`
- `SERVER MEMBERS INTENT`

`src/discord_bot.rs` では `MESSAGE_CONTENT` と `GUILD_MEMBERS` を要求しているため、ここが無効だと正常に動きません。

### 1-3. Bot をサーバーへ招待

OAuth2 URL Generator で以下を選びます。

Scopes:

- `bot`

Bot Permissions の最低限:

- `View Channels`
- `Send Messages`
- `Read Message History`
- `Create Public Threads` は不要
- `Create Private Threads` は不要
- `Send Messages in Threads`

この Bot は既存スレッド内の投稿を読む前提なので、主に「スレッド内の読み書き」ができれば十分です。

### 1-4. 実行許可の制御

Bot は allowlist 方式です。実行を許可するには、次のどちらかを設定する必要があります。

- `DISCORD_ALLOWED_USER_IDS`
- `DISCORD_ALLOWED_ROLE_IDS`

両方空だと起動時にエラーになります。

#### ユーザー ID を使う場合

1. Discord で Developer Mode を有効化する
2. 許可したいユーザーを右クリックして ID をコピーする
3. カンマ区切りで `DISCORD_ALLOWED_USER_IDS` に設定する

```text
DISCORD_ALLOWED_USER_IDS=123456789012345678,234567890123456789
```

#### ロール ID を使う場合

1. Discord で対象ロールの ID を取得する
2. カンマ区切りで `DISCORD_ALLOWED_ROLE_IDS` に設定する

```text
DISCORD_ALLOWED_ROLE_IDS=345678901234567890,456789012345678901
```

#### 推奨

通常運用では個人 ID よりロール ID を使う方が管理しやすいです。

## 2. Discord 側の利用前提

Bot は以下の条件を満たす投稿だけを処理します。

- DM ではない
- Discord サーバー内のメッセージである
- スレッド内の投稿である
- allowlist に含まれるユーザーまたはロールによる投稿である

つまり、通常のテキストチャンネル直下の投稿は処理されません。依頼はスレッド内で行ってください。

また、現状の v1 では `research` タスクのみ実行し、`coding` と判定される内容は拒否されます。

## 3. Notion 設定

### 3-1. Notion integration 作成

1. Notion の My integrations を開く
2. 新しい integration を作成する
3. 名前を設定する
4. Internal integration token を取得する

取得した token は `NOTION_TOKEN` に設定します。

```text
NOTION_TOKEN=secret_xxx
```

### 3-2. Task Database 作成

Notion に task 管理用の database を作成し、以下のプロパティ名を正確に用意してください。

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

型と名前がずれると、Bot や public app が期待どおり動きません。

### 3-3. Database を integration に共有

database ページで `Share` を開き、作成した integration を追加してください。

共有していない場合は、Notion API で 401 または 404 相当のエラーになります。

### 3-4. Database ID の取得

Task Database の URL から database ID を取得し、`NOTION_TASK_DATABASE_ID` に設定します。

```text
NOTION_TASK_DATABASE_ID=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```

## 4. Bot 環境変数の設定

最低限必要な環境変数は以下です。

```text
DISCORD_TOKEN=xxxxxxxxxxxxxxxx
DISCORD_ALLOWED_ROLE_IDS=345678901234567890
NOTION_TOKEN=secret_xxx
NOTION_TASK_DATABASE_ID=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
PUBLIC_BASE_URL=https://your-project.vercel.app
```

必要に応じて追加で設定します。

```text
SQLITE_PATH=data/discord-agent.sqlite3
CODEX_BIN=codex
CODEX_MODEL=
WORKER_CONCURRENCY=1
DISCORD_ALLOWED_USER_IDS=
```

補足:

- `PUBLIC_BASE_URL` は Vercel 側の公開 URL に合わせる
- これにより Notion の `Public URL` が `https://<domain>/tasks/<task_id>` で記録される
- `WORKER_CONCURRENCY` は同時実行数

## 5. ローカルまたは運用環境での確認

Bot 側のテストはコンテナ内で実行します。

```bash
docker compose build
docker compose run --rm app cargo test
```

起動後の確認ポイント:

- Bot が Discord に online 表示される
- 許可されたユーザーがスレッド内で投稿すると `Accepted task` が返る
- 完了後、Notion にページが作成される
- `Publish=true` の完了タスクに `Public URL` が入る

## 6. よくある失敗

### `DISCORD_TOKEN is required`

原因:

- `DISCORD_TOKEN` が未設定

対応:

- Bot token を再取得して設定する

### `either DISCORD_ALLOWED_USER_IDS or DISCORD_ALLOWED_ROLE_IDS must be set`

原因:

- allowlist が空

対応:

- ユーザー ID かロール ID を少なくとも 1 つ設定する

### Bot がメッセージに反応しない

原因候補:

- スレッド外の投稿をしている
- allowlist 対象外のユーザー
- `MESSAGE CONTENT INTENT` が無効
- `SERVER MEMBERS INTENT` が無効

対応:

- スレッド内で投稿する
- allowlist を確認する
- Developer Portal で intents を有効にする

### Notion にページが作成されない

原因候補:

- `NOTION_TOKEN` が誤っている
- integration が database に共有されていない
- `NOTION_TASK_DATABASE_ID` が誤っている
- database のプロパティ名が違う

対応:

- token と database ID を再確認する
- `Share` 設定を確認する
- プロパティ名と型を README どおりに揃える

### 公開 URL が古いドメインのまま

原因:

- Bot 側の `PUBLIC_BASE_URL` が古い

対応:

- Bot の `PUBLIC_BASE_URL` を最新の Vercel URL に更新する

## 7. 運用メモ

- Discord Bot と Vercel public app は別デプロイ
- Notion は Bot と public app の共通データソース
- Discord の allowlist は運用ポリシーに合わせてロール中心で管理するとよい
- Notion の property 名はコードに埋め込まれているため、変更時は実装変更も必要