# Sample Run: Cloud Run Observability Skill (`sitebookify` profile)

Generated at: 2026-02-12
Input profile: `sitebookify`
Related task: `https://trello.com/c/Xvlu1U1f`

## Incident Summary

- Target: `project_id=<unknown>`, `region=<unknown>`, `target_type=service`, `target_name=sitebookify`
- Time window: 2026-02-12T00:00:00Z to 2026-02-12T01:00:00Z (default window for sample)
- User-visible impact: `create_job` accepted後に処理が継続しない可能性があり、ジョブ完了が不安定になる。
- Current status: ongoing (runtime evidence不足のため未確定)

## Evidence Table

| Evidence ID | Timestamp (UTC) | Source | Signal | Confidence | Raw Ref |
|---|---|---|---|---|---|
| ev-001 | 2026-02-12T00:29:49.963Z | cloud_logging | 調査カードに「create_job直後に処理継続しない/停止する事象」が記載されている。 | medium | `https://trello.com/c/Xvlu1U1f` |
| ev-002 | 2026-02-12T00:00:00Z | cloud_run_revision | `create_job` 内で `run_job` が `queue.spawn` により非同期起動される。 | high | `src/bin/sitebookify-app.rs:484` |
| ev-003 | 2026-02-12T00:00:00Z | cloud_run_revision | queue実装は `tokio::spawn` の in-process 実行で、外部永続キュー記述がない。 | high | `src/app/queue.rs:19` |
| ev-004 | 2026-02-12T00:00:00Z | cloud_run_revision | Cloud Run 定義は `max_instance_count` と `max_instance_request_concurrency` を設定。 | high | `infra/terraform/cloudrun-public-gcs/main.tf:103` |
| ev-005 | 2026-02-12T00:00:00Z | cloud_run_revision | コンテナ起動引数は `0.0.0.0:8080` で bind 指定済み。 | high | `Dockerfile:73` |

## Root Cause Candidates

1. Background task model is vulnerable to Cloud Run instance lifecycle interruption
   - Confidence: medium
   - Rationale: `create_job` でジョブ受付後に in-process 非同期処理へ切り替えており、ランタイム継続保証の根拠が不足している。
   - Supporting evidence: ev-001, ev-002, ev-003
2. Runtime scaling/concurrency assumptions may not match asynchronous execution model
   - Confidence: low
   - Rationale: Cloud Run 側のスケーリング設定はあるが、バックグラウンド実行継続性との整合は未確認。
   - Supporting evidence: ev-003, ev-004
3. Port or bind mismatch
   - Confidence: low
   - Rationale: 起動引数は `0.0.0.0:8080` で整合しており、主因である可能性は低い。
   - Supporting evidence: ev-005

## Fix Plan

### Immediate (Short-Term)

1. `create_job` 後の実行継続可否を明示的に検証するログを追加する
   - Expected impact: request完了後の中断ポイントを特定しやすくなる
   - Risk: ログ量増加
   - Addresses: Candidate 1, Candidate 2
2. 障害再現時に revision 終了理由を同時に採取する運用手順を固定する
   - Expected impact: 推測ベースの議論を減らせる
   - Risk: 運用手順の徹底コスト
   - Addresses: Candidate 1, Candidate 2

### Permanent (Long-Term)

1. in-process 非同期実行から、インスタンス寿命に依存しない実行モデルへ移行する
   - Expected impact: `create_job` 受付後の実行信頼性を向上
   - Risk: 実装変更と運用設計の工数増
   - Addresses: Candidate 1, Candidate 2
2. 実行モデル変更後にジョブ状態遷移の整合性監視を導入する
   - Expected impact: 退行の早期検知
   - Risk: 監視設計・調整コスト
   - Addresses: Candidate 1

## Prevention Plan

1. Metric: Job completion ratio
   - Condition: `create_job` accepted件数に対する `DONE` 到達件数
   - Threshold: 5分移動窓で 99% 未満
   - Rationale: 受付後に実行が失われる回帰を検知できる
2. Metric: Job stuck duration
   - Condition: `QUEUED` または `RUNNING` 継続時間
   - Threshold: p95 が 15分超
   - Rationale: 中断・ハングを早期検知できる
3. Metric: Runtime termination signals
   - Condition: revision 終了イベントの異常増加
   - Threshold: 10分窓で通常比 3倍超
   - Rationale: インスタンス寿命起因の中断を検知しやすい

## Data Gaps and Follow-Ups

- Missing signal: Cloud Logging の実ランタイムログ（stderr/stdout, termination reason）
- Why missing: このサンプル実行では GCP 実環境ログへアクセスしていない
- Follow-up: 実運用プロジェクトIDとリージョンを指定し、同一時間窓で logs/revision/job execution を再収集する
