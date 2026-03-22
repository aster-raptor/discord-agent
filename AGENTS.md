# AGENTS.md

## Execution policy
- このリポジトリでは、ホストOSに依存を入れて実行しないこと。
- ビルド、テスト、lint、開発用コマンドは必ず Docker Compose 経由で実行すること。
- ホスト上で `python`, `pytest`, `node`, `npm`, `pnpm` などを直接実行しないこと。

## Allowed commands
- `docker compose build`
- `docker compose up -d`
- `docker compose run --rm app <command>`
- `docker compose exec app <command>`

## Validation policy
- コード変更後は必ずコンテナ内でテストまたは lint を実行すること。
- 失敗した場合は、失敗ログを確認し、原因を修正して再実行すること。

## Safety
- 破壊的な Docker コマンド（volume 削除、system prune など）は必要性を説明してから行うこと。