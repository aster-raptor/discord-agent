---
name: container-dev
description: Docker Compose 経由で開発・テスト・検証を行うためのワークフロー。ホストでは依存コマンドを直接実行せず、コンテナ内で確認する。
---

# Container Dev Skill

## 目的
このスキルは、Codex がホスト上でコード編集を行いながら、
ビルド・テスト・lint・デバッグを Docker Compose 経由で実施するための手順を提供する。

## 基本方針
- ホストではソース編集のみを行う。
- 実行検証は必ずコンテナ内で行う。
- 直接 `python`, `pytest`, `node`, `npm`, `pnpm` をホストで叩かない。
- 変更後は必ず何らかの検証を行う。

## 標準フロー
1. 関連ファイルを確認する
2. 必要なコード変更を行う
3. 必要に応じて `docker compose build` を実行する
4. テストまたは lint を `docker compose run --rm ...` もしくは `docker compose exec ...` で実行する
5. エラーがあればログを読んで修正する
6. 最後に、何を変更し、どのコンテナコマンドで検証したかを報告する

## 推奨コマンド例
- `docker compose build app`
- `docker compose up -d app db`
- `docker compose run --rm app pytest`
- `docker compose run --rm app npm test`
- `docker compose exec app bash`

## 禁止事項
- ホストで依存付き実行をしない
- 根拠なくコンテナや volume を削除しない
- 必要以上に全サービスを再起動しない

## 完了条件
- 変更内容が反映されている
- 少なくとも1つ以上の妥当な検証コマンドをコンテナ内で実行している
- 失敗した場合は失敗理由を報告している