# ExecPlan: Cloud Run Worker 分離で sitebookify 実行安定化

## Goal

- `CreateJob` 受付後のジョブ実行を API プロセス寿命から分離し、Cloud Run のインスタンス再作成や OOM の影響を受けにくくする。
- `GetJob` ポーリング負荷が高い状況でも、ジョブ完走率を維持できる構成へ移行する。
- 実装順序と受け入れ条件を固定し、実装者が追加判断なしで進められるようにする。

### Non-goals

- 認証/課金/公開設定など、アプリ外のプロダクト仕様変更は扱わない。
- 初回で Cloud Run Jobs + Cloud Tasks/PubSub の全面導入までは行わない。
- 既存 proto 契約（CreateJob/GetJob/GenerateJobDownloadUrl）の破壊的変更は行わない。

## Scope

### In scope

- 実行責務分離のためのアプリ改修。
  - `CreateJob` から direct `tokio::spawn` を除去。
  - `JobDispatcher` 抽象を導入。
  - Worker 実行経路を新設。
- Cloud Run Worker 運用に必要な最小インフラ改修。
  - Worker サービス（または同等起動経路）追加。
  - IAM/環境変数/デプロイ設定を追加。
- 観測性と運用受け入れ条件の追加。
  - 429/5xx/OOM/完走率を定点観測できる形にする。

### Out of scope

- Job 状態ストアの全面 DB 化。
- UI の大幅刷新。
- 全ジョブ履歴のデータ移行機構。

### 変更対象

- `src/bin/sitebookify-app.rs`
- `src/app/queue.rs`（必要なら縮退）
- `src/app/` 配下に dispatcher/worker 実装ファイル追加
- `infra/terraform/cloudrun-public-gcs/`（worker 用リソース追加）
- `tests/`（integration シナリオ追加）

## Milestones

1. 実行抽象の導入
   - `JobDispatcher` を導入し、`CreateJob` は `dispatch(job_id)` を呼ぶだけにする。
   - 観測可能な成果: `src/bin/sitebookify-app.rs` の `create_job` から `queue.spawn` 呼び出しが消える。

2. Worker 実行経路の追加
   - `job_id` を受けて `runner.run_job(job_id)` を実行する Worker エントリポイントを追加する。
   - 観測可能な成果: ローカルで worker 単体実行し、`queued -> running -> done` を確認できる。

3. API から Worker 起動へ接続
   - `SITEBOOKIFY_EXECUTION_MODE` で `inprocess/worker` 切替可能にする。
   - 観測可能な成果: `execution_mode=worker` で `CreateJob` 後に worker 側ログへ同一 `job_id` が出力される。

4. Cloud Run インフラ反映
   - Worker 用 Cloud Run 設定（サービス、IAM、env）を Terraform へ追加する。
   - 観測可能な成果: `terraform plan` が意図した差分のみを出し、apply 後に worker 起動経路が動く。

5. 安定性検証と回帰確認
   - 高頻度 `GetJob` と重いジョブ実行を行い、429/504/OOM の再発状況を評価する。
   - 観測可能な成果: 受け入れ基準を満たす測定結果を `Progress` に記録する。

## Tests

### Integration-first シナリオ

- `CreateJob` が 200 を返し、非同期に worker 側で実行開始される。
- `GetJob` 連続ポーリング下で状態遷移が壊れない（`queued -> running -> done|error`）。
- Worker 側で失敗した場合、`error` 状態とエラーメッセージが保存される。
- 既存機能の回帰がない。
  - `/preview`
  - `GenerateJobDownloadUrl`
  - `/jobs/:id/book.md`
  - `/jobs/:id/book.epub`

### Cloud Run 検証

- `run.googleapis.com/requests` で 429/5xx の推移を比較する。
- `run.googleapis.com/varlog/system` で OOM メッセージ再発有無を確認する。
- 完走率（CreateJob 受付に対する done 到達率）を比較する。

## Decisions / Risks

### Decisions

- `J8RjepKC` の A 案（API/Worker 分離）を採用する。
- B 案（設定調整のみ）は補助策扱いにし、主軸にはしない。
- 将来の Cloud Run Jobs 化を妨げないため、dispatch 抽象を先に入れる。

### Risks

- Worker 呼び出し失敗時にジョブが取り残される。
- 実行経路の二重化により二重実行のリスクが出る。
- 運用の複雑化で障害解析が遅くなる。

### Mitigations

- dispatch 失敗時はジョブを明示的に `error` へ遷移させる。
- `running` 中の再実行ガードを入れる。
- `job_id` 相関ログを API/Worker で統一する。

## Progress

- 2026-02-12: `Xvlu1U1f` と実ログを突合し、`504/429` 連発と `Memory limit 512MiB exceeded` を確認した。
- 2026-02-12: `J8RjepKC` の方針選定として A 案採用を確定した。
- 2026-02-12: 本 ExecPlan を作成し、実装順序と受け入れ基準を固定した。
- 2026-02-12: Phase1 の着手として `JobDispatcher` と `SITEBOOKIFY_EXECUTION_MODE` を導入し、`CreateJob` の direct spawn を dispatcher 経由へ置換した。
- 2026-02-12: Phase2 として Worker 起動経路を追加した。`/internal/jobs/:job_id/run` を導入し、`worker` モード時は API から `SITEBOOKIFY_WORKER_URL` へジョブ実行を委譲する構成にした。
- 2026-02-12: Job 状態ストアをローカル FS と GCS で共通化した。`JobStore::list_job_ids` を追加し、Cloud Run の複数サービス構成でも `ListJobs` が同じストアを読めるようにした。
- 2026-02-12: Terraform に API/Worker の 2 サービス構成を追加し、`execution_mode` と `worker_auth_token` で切替可能にした。
- 2026-02-12: 変更後の検証として `cargo fmt --all` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test --all` / `terraform validate` を実行し、全て成功した。
