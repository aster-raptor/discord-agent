# discord-agent アーキテクチャ

## 概要

`discord-agent` は、Discord スレッドに投稿された依頼を受け付け、Codex CLI で処理し、その結果を Notion と RSS に公開する Rust 製のシステムです。実行系は 2 つのバイナリに分かれています。

- `bot`: Discord から依頼を受け付け、SQLite に保存し、ワーカーで Codex を実行する
- `rss`: Cloud Run 上で動作し、Notion に公開済みのタスクを読み出して RSS フィードと公開ページを配信する

## システム全体像

```text
Discord user
  |
  v
Discord thread message
  |
  v
bot binary (serenity)
  |- allowlist check
  |- task type inference
  |- SQLite persistence
  `- in-memory queue (tokio mpsc)
        |
        v
   worker loop
        |
        v
   Codex CLI
        |
        v
   result summarization
        |
        v
   Notion database
        |
        +--> public task page metadata
        |
        `--> rss binary on Cloud Run (axum) queries published tasks from Notion
                 |- /rss.xml
                 |- /tasks/:task_id
                 `- /healthz
```

## コンポーネント構成

### 1. エントリポイント

- `src/bin/bot.rs`
  Discord Bot プロセスの起動点です。環境変数を読み込み、ロギングを初期化し、`discord_bot::run` を呼びます。
- `src/bin/rss.rs`
  RSS/公開ページ配信プロセスの起動点です。環境変数を読み込み、`rss_server::run` を呼びます。
- `src/main.rs`
  補助的な案内メッセージのみを持ち、実運用では `bot` または `rss` を明示して起動する前提です。

### 2. 設定管理

`src/config.rs` の `AppConfig` が全設定を集約します。主な責務は以下です。

- 環境変数の読込
- Discord Bot 用の必須設定チェック
- RSS サービス用の必須設定チェック
- allowlist や並列ワーカー数などのランタイム設定の保持

1 つの設定構造体を `bot` と `rss` の両方で共有しているため、共通設定と実行モード固有設定が同じ層で管理されています。`rss` 実行時は Cloud Run 標準の `PORT` を優先して bind します。

### 3. ドメインモデル

`src/models.rs` にはシステム内で扱う主要なデータ構造があります。

- `TaskType`
  現状は `Research` と `Coding` を定義していますが、実際に処理されるのは `Research` のみです。
- `TaskStatus`
  受理から完了までの状態遷移を表します。
- `TaskRecord`
  タスク本体です。Discord メタデータ、入力プロンプト、出力、公開状態、Notion ページ ID、タイムスタンプを保持します。
- `PublicTaskSummary`
  RSS や公開ページに必要な最小限の公開情報です。

このモデルにより、内部処理用の完全な記録と、外部公開用の要約データが分離されています。

### 4. 永続化層

`src/db.rs` の `Database` は SQLite をラップします。接続は `Mutex<Connection>` で保護され、単一プロセス内で直列化されたアクセスを行います。

テーブル構成:

- `tasks`
  タスクの正本
- `task_messages`
  受付時の元メッセージ保存
- `task_events`
  ステータス遷移の監査ログ
- `approvals`
  将来の承認フロー用テーブル

アーキテクチャ上のポイント:

- アプリ起動時にスキーマを自動初期化する
- Discord から受けた依頼はまず SQLite に保存してからキュー投入する
- ワーカー処理中の状態更新も SQLite に記録する
- 現状の RSS 配信は SQLite ではなく Notion を参照する

### 5. Discord 受付層

`src/discord_bot.rs` が Discord 側の入口です。`serenity` を用いたイベント駆動構成になっています。

受付処理の流れ:

1. `message` イベントを受信
2. Bot 自身の投稿と DM を除外
3. スレッド内メッセージかどうかを判定
4. ユーザー ID / ロール ID の allowlist を検証
5. 投稿文面からタスク種別を推定
6. タイトルと実行プロンプトを生成
7. `TaskRecord` を SQLite に保存
8. ステータスを `queued` に更新
9. `tokio::mpsc` キューに `TaskJob` を投入
10. Discord に受理メッセージを返す

補助機能:

- URL を抽出してプロンプト末尾へ付加する
- 投稿文の先頭行からタイトルを自動生成する
- `DiscordProgressReporter` が進捗メッセージを同じスレッドへ返す

### 6. ワーカー実行層

同じく `src/discord_bot.rs` にワーカー処理があります。起動時に `WORKER_CONCURRENCY` 分だけタスクを spawn し、共有キューからジョブを取り出します。

ワーカーの責務:

- キューから `task_id` を取得
- SQLite から `TaskRecord` を再読込
- ステータスを `running` に更新
- `CodexRunner` を使って Codex CLI を実行
- 成功時は要約を生成し、Notion に公開し、`completed` に更新
- 失敗時はエラーを保存し、`failed` に更新

この構成により、Discord のイベント受付と Codex の実行時間を分離しています。一方で、キューはプロセス内メモリのみなので、Bot プロセス再起動時に未処理ジョブは失われます。

### 7. Codex 実行アダプタ

`src/codex.rs` の `CodexRunner` は外部プロセスとして Codex CLI を呼び出します。

特徴:

- `codex exec <prompt>` の形式で起動する
- 必要に応じて `--model` を付与する
- stdout/stderr を回収して `CodexOutput` に格納する
- 標準出力も標準エラーも空の場合は異常として扱う

このモジュールは LLM 実行の詳細を 1 箇所に閉じ込めるアダプタ層として機能します。Bot 本体は「外部実行して結果を受け取る」ことだけを前提にしています。

### 8. Notion 連携層

`src/notion.rs` の `NotionClient` は Notion API との通信を担当します。

主な責務:

- 認証ヘッダと API バージョンの設定
- タスク完了時の Notion ページ作成
- `Publish=true` のタスク一覧取得
- 指定タスク ID の公開情報取得

ページ作成時には、Task DB のプロパティに加えて以下の本文ブロックを作ります。

- Task Summary
- Original Prompt
- Codex Output

公開 URL は `PUBLIC_BASE_URL/tasks/:task_id` の形で Notion 側にも書き戻されます。

### 9. 公開配信層

`src/rss_server.rs` は `axum` ベースの軽量 HTTP サーバーです。Cloud Run 配備を前提とした stateless な配信層として動作します。

提供エンドポイント:

- `/healthz`
  ヘルスチェック
- `/rss.xml`
  公開済みタスクの RSS フィード
- `/tasks/:task_id`
  個別タスクの簡易 HTML ページ

データソースは SQLite ではなく Notion です。つまり、公開面は Notion を一次情報源とみなす構成になっています。Cloud Run 上の `rss` サービスも GCS を介さず Notion API を直接参照します。

## 主要データフロー

### タスク受付から公開まで

1. Discord スレッドに依頼を書き込む
2. Bot が allowlist とスレッド種別を検証する
3. Bot がタスクを SQLite に保存する
4. Bot がジョブをメモリキューへ投入する
5. ワーカーが Codex CLI を呼び出す
6. 成功した場合、要約と生出力を整形する
7. Notion にページを作成し、公開プロパティを更新する
8. Cloud Run 上の RSS サービスが Notion を参照して一覧・詳細を配信する

### 失敗時の流れ

1. Codex 実行失敗または Notion 連携失敗が発生する
2. ワーカーが `failed` を SQLite に保存する
3. Discord スレッドへ失敗メッセージを返す
4. 公開対象には載らない

## 状態遷移

実装上の代表的な状態遷移は以下です。

```text
accepted
  -> queued
  -> running
  -> summarizing
  -> completed

accepted/queued/running/summarizing
  -> failed
```

`awaiting_approval` と `rejected` はモデル上は定義済みですが、現状の実行パスでは使われていません。

## 外部依存

- Discord API
  依頼受付と進捗通知
- Codex CLI
  研究タスクの実処理
- SQLite
  内部状態の永続化
- Notion API
  完了タスクの公開先
- Axum HTTP サーバー
  RSS と公開ページの配信

## 設計上の意図

- Discord 側は「依頼受付と進捗通知」に集中する
- 長時間処理はワーカーに切り出してイベントループを塞がない
- 内部記録は SQLite、外部公開は Notion という責務分担にする
- 配信面は RSS サービスとして分離し、Bot と独立運用できるようにする

## 現状の制約と今後の論点

- Coding タスク型は定義済みだが、v1 では受け付けず Research のみ処理する
- 承認フロー用の状態とテーブルはあるが未接続
- ジョブキューがプロセス内メモリのみのため、再起動耐性がない
- 公開面が Notion 依存なので、Notion 障害時は Cloud Run 上の RSS も影響を受ける
- SQLite には公開用一覧取得 API があるが、現状の RSS サーバーはそれを使っていない
- DB 接続は単一 `Connection` を `Mutex` で保護しており、高並列化には向かない

## 読むべき実装ファイル

- `src/bin/bot.rs`
- `src/bin/rss.rs`
- `src/config.rs`
- `src/discord_bot.rs`
- `src/codex.rs`
- `src/db.rs`
- `src/notion.rs`
- `src/rss_server.rs`
- `src/models.rs`
