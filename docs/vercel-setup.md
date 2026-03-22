# Vercel 設定手順

## 概要

このドキュメントは、`discord-agent` の公開面を Vercel 上で動かすための設定手順です。対象は Next.js/TypeScript の public app で、Rust 製の `bot` は別環境で動かす前提です。

公開アプリの役割:

- `/healthz` を返す
- `/rss.xml` を返す
- `/tasks/:task_id` を返す
- 公開元データとして Notion API を直接参照する

## 前提条件

事前に以下を用意してください。

- Vercel アカウント
- GitHub 上のこのリポジトリへのアクセス
- Notion integration token
- 公開対象タスクを保存する Notion database
- Rust Bot 側の運用環境

Notion database には README に記載している以下のプロパティが必要です。

- `Task ID`
- `Title`
- `Status`
- `Task Type`
- `Publish`
- `Public Summary`
- `Updated At`
- `Completed At`
- `Thread ID`
- `Public URL`

## 1. ローカル確認

Vercel に接続する前に、コンテナ内で public app がビルドできることを確認します。

```bash
docker compose build
docker compose run --rm app npm install
docker compose run --rm app npm test
docker compose run --rm app npm run build
```

開発サーバを動かす場合:

```bash
docker compose up -d
docker compose exec app npm run dev
```

既定のローカル URL は `http://localhost:3000` です。

## 2. Vercel プロジェクト作成

1. Vercel ダッシュボードで `Add New Project` を選ぶ
2. この GitHub リポジトリを import する
3. Framework Preset は `Next.js` を選ぶ
4. Root Directory はリポジトリ直下のままにする
5. Build Command は既定の `next build` を使う
6. Output Directory は既定のままにする
7. Install Command は既定の `npm install` を使う

このリポジトリでは public app がルートにあるため、サブディレクトリ指定は不要です。

## 3. 環境変数設定

Vercel の Project Settings で以下を設定します。

- `NOTION_TOKEN`
  public app が Notion API を読むための integration token
- `NOTION_TASK_DATABASE_ID`
  公開対象タスクが入っている database ID
- `PUBLIC_BASE_URL`
  Vercel の公開 URL

設定例:

```text
NOTION_TOKEN=secret_xxx
NOTION_TASK_DATABASE_ID=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
PUBLIC_BASE_URL=https://your-project.vercel.app
```

注意点:

- `PUBLIC_BASE_URL` は `https://` 付きの完全 URL にする
- Custom Domain を使う場合は、そのドメインに切り替えた後の URL を設定する
- Preview 環境と Production 環境で URL を分けたい場合は、Environment ごとに設定する

## 4. 初回デプロイ

環境変数を保存したら deploy を実行します。初回 deploy 後、以下を確認します。

- `https://<your-domain>/healthz` が `ok` を返す
- `https://<your-domain>/rss.xml` が 200 を返す
- `Publish=true` のタスクが Notion に存在する場合、RSS に item が含まれる
- `https://<your-domain>/tasks/<task_id>` が HTML を返す

## 5. Bot 側設定の切り替え

Rust Bot は Notion に `Public URL` を書き戻すため、Bot 側の `PUBLIC_BASE_URL` も Vercel の公開 URL に合わせる必要があります。

Bot 側で設定する値:

```text
PUBLIC_BASE_URL=https://your-project.vercel.app
```

この変更後に完了したタスクから、Notion の `Public URL` が Vercel ドメインを指すようになります。

## 6. Custom Domain を使う場合

1. Vercel の Project Settings で Domain を追加する
2. DNS を Vercel 指示どおり設定する
3. Domain が有効化されたら `PUBLIC_BASE_URL` を Custom Domain に更新する
4. Bot 側の `PUBLIC_BASE_URL` も同じ値に更新する

更新後の例:

```text
PUBLIC_BASE_URL=https://rss.example.com
```

## 7. デプロイ後確認

最低限、以下を確認してください。

- `healthz` が 200 を返す
- `rss.xml` の `link` と各 item の URL が `PUBLIC_BASE_URL` を使っている
- `tasks/:task_id` のページが文字化けせず表示される
- Notion API の認証エラーが出ていない

RSS の内容確認例:

- `<channel><link>` が公開ドメインを指している
- `<item><link>` が `https://<domain>/tasks/<task_id>` になっている

## 8. よくある失敗

### `failed to query notion: 401`

原因:

- `NOTION_TOKEN` が誤っている
- Integration が対象 database に共有されていない

対応:

- Token を再確認する
- Notion database を integration に共有する

### `failed to query notion: 404`

原因:

- `NOTION_TASK_DATABASE_ID` が誤っている

対応:

- database ID を再取得して設定し直す

### RSS に何も出ない

原因:

- `Publish=true` の完了タスクがない
- `Task ID` や `Updated At` など必須プロパティが欠けている

対応:

- Notion の公開対象ページとプロパティ名を確認する

### Notion の `Public URL` が古いドメインのまま

原因:

- Bot 側の `PUBLIC_BASE_URL` が古い

対応:

- Bot の環境変数を Vercel の公開 URL に更新する

## 9. 運用メモ

- public app は stateless なので、公開面の正本は Notion
- Vercel 側に SQLite は不要
- Bot と public app は別々にデプロイ、更新してよい
- 公開 URL を変えた場合は、Vercel と Bot の両方で `PUBLIC_BASE_URL` を揃える

