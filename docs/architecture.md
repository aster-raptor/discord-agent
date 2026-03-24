# discord-agent アーキテクチャ

## 概要

`discord-agent` は、Discord の Slash Command で受け付けた依頼を Rust 製の Bot が処理し、Codex CLI で実行した結果を Notion に保存します。公開面は Rust とは分離されており、Vercel 上の Next.js/TypeScript アプリが Notion を直接参照して RSS と公開ページを配信します。

- `bot`: Discord の `/research` 依頼を受け付け、SQLite に保存し、ワーカーで Codex を実行する Rust プロセス
- `public app`: Vercel 上で動作し、Notion に公開済みのタスクを読み出して `/rss.xml` と `/tasks/:task_id` を返す Next.js アプリ

## システム全体像

```text
Discord user
  |
  v
Discord slash command
  |
  v
bot binary (Rust / serenity)
  |- slash command handling
  |- SQLite persistence
  `- worker loop
        |
        v
   Codex CLI
        |
        v
   Notion database
        |
        +--> public task page metadata
        |
        `--> Vercel / Next.js public app
                 |- /healthz
                 |- /rss.xml
                 `- /tasks/:task_id
```

## コンポーネント構成

### 1. Rust Bot

Rust 側の責務は Discord 受付とバックグラウンド実行です。

- `src/bin/bot.rs`
  Bot プロセスの起動点
- `src/discord_bot.rs`
  Slash Command の受付、進捗通知、ワーカー実行
- `src/db.rs`
  SQLite 永続化
- `src/codex.rs`
  Codex CLI 呼び出し
- `src/notion.rs`
  完了タスクを Notion へ保存

Bot は単一ユーザー利用を前提としており、Discord 内の追加認可制御は行いません。依頼の起点は Slash Command に限定し、通常メッセージは task を作らず使い方案内だけを返します。完了タスクは `PUBLIC_BASE_URL/tasks/:task_id` の形式で公開 URL を Notion に記録します。

### 2. Next.js Public App

公開面は App Router の Route Handlers で実装しています。

- `app/healthz/route.ts`
  ヘルスチェック
- `app/rss.xml/route.ts`
  Notion から公開済みタスクを取得し、RSS XML を返す
- `app/tasks/[task_id]/route.ts`
  個別タスクの HTML ページを返す
- `lib/notion.ts`
  Notion API から公開済みタスクを取得する TypeScript 実装
- `lib/public-tasks.ts`
  RSS/XML と HTML のレンダリング処理

Next.js 側は stateless で、公開データの正本は Notion です。SQLite は参照しません。

### 3. 設定管理

主要な設定は以下です。

- Rust Bot
  `DISCORD_TOKEN`, `SQLITE_PATH`, `CODEX_BIN`, `CODEX_MODEL`, `WORKER_CONCURRENCY`, `NOTION_TOKEN`, `NOTION_TASK_DATABASE_ID`, `PUBLIC_BASE_URL`
- Next.js Public App
  `NOTION_TOKEN`, `NOTION_TASK_DATABASE_ID`, `PUBLIC_BASE_URL`

`PUBLIC_BASE_URL` は Vercel 上の公開 URL を指し、Bot が Notion に書き戻すリンク生成にも使われます。

## 主要データフロー

### タスク受付から公開まで

1. Discord のスレッドで `/research` を実行する
2. Rust Bot が command を受け付け、スレッドかどうかを検証する
3. Bot がタスクを SQLite に保存する
4. ワーカーが Codex CLI を呼び出す
5. 完了結果を Bot が Notion に保存する
6. Vercel 上の Next.js public app が Notion を参照して `/rss.xml` と `/tasks/:task_id` を返す

### 失敗時の流れ

1. Codex 実行失敗または Notion 連携失敗が発生する
2. Bot が `failed` を SQLite に保存する
3. Discord スレッドへ失敗メッセージを返す
4. Notion に公開されないため RSS にも載らない

## 外部依存

- Discord API
  依頼受付と進捗通知
- Codex CLI
  調査タスクの実処理
- SQLite
  Bot 内部状態の永続化
- Notion API
  完了タスクの公開元データ
- Vercel / Next.js
  RSS と公開ページの配信

## 現状の制約

- `/research` は Discord のサーバー内スレッドのみで実行し、DM は扱わない
- 通常メッセージは task を作らず、使い方だけを返す
- v1 では /research による Research タスクのみ処理する
- 公開面は Notion 依存なので、Notion 障害時は RSS と公開ページも影響を受ける
- ジョブキューがプロセス内メモリのみのため、Bot 再起動時に未処理ジョブは失われる
- SQLite は Bot の内部記録用であり、公開面のデータソースには使っていない