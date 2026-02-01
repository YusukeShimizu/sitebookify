# ExecPlan: Cloud Run 公開実行と GCS 保存（URL 1 時間 / オブジェクト 1 日）

## Goal

- `sitebookify-app` を Cloud Run にデプロイし、一般ユーザがブラウザから実行できるようにする。
- 生成物は GCS に保存し、ダウンロードは 1 時間で失効する URL で提供する。
- 生成物オブジェクトは GCS ライフサイクルで 1 日後に削除する。
- ログインと永続 DB を不要にする。

### Non-goals

- ユーザ認証、課金、利用規約同意などのフローは扱わない。
- ジョブ履歴の永続化と閲覧は扱わない。
- 任意 URL を安全に扱えることは保証しない。

## Scope

### In scope

- GCP リソース設計と最小手順。
  - Artifact Registry。
  - Cloud Run（公開 HTTP）。
  - GCS バケット（生成物保管）。
  - IAM とサービスアカウント。
  - 期限切れ生成物の削除方式。
- CI/CD 方針。
  - GitHub Actions からの deploy 方針。
  - 必要な Variables と IAM 権限。
- アプリ設定の方針。
  - バケット名、URL 有効期限、保存プレフィックス。
  - 実行制限（時間、ページ数、並列数）。

### Out of scope

- Cloud Armor や WAF の本格設定。
- カスタムドメインと TLS 証明書の運用。
- 高度な濫用対策（CAPTCHA、課金、利用者単位の quota）。

## Milestones

1. **アーキテクチャを確定する。**
   - 公開 Cloud Run の実行モデルを確定する。
   - 「ダウンロード URL 1 時間 / オブジェクト 1 日削除」の定義を確定する。
2. **GCP リソースを用意する。**
   - Artifact Registry と GCS バケットを作る。
   - Cloud Run の実行用サービスアカウントを作る。
   - 最小権限の IAM を付与する。
3. **Cloud Run へデプロイできるようにする。**
   - 公開アクセスを有効にする。
   - `/healthz` を外部から確認できる。
4. **ダウンロード URL の 1 時間失効を満たす。**
   - 署名付き URL の `expires=3600` を満たす。
5. **GCS の 1 日削除を満たす。**
   - GCS ライフサイクルで `age=1` の Delete を設定する。
   - 1 日を超えた生成物が消える。
6. **CI/CD を整備する。**
   - `main` への push で Cloud Run へデプロイできる。
   - 失敗時にロールバックできる手順を残す。

## Tests

### Local

- `nix develop -c just ci` を通す。
- `docker build -t sitebookify-app:local .` を通す。
- `docker run --rm -p 18080:8080 sitebookify-app:local` を実行する。
- `curl -fsS http://127.0.0.1:18080/healthz` が `ok` を返す。

### Deploy smoke

- Cloud Run の URL に対して `GET /healthz` が `200` を返す。
- 既知の小さい URL を入力して生成が完走する。
- GCS に zip が作られ、署名付き URL でダウンロードできる。

### TTL

- 署名付き URL の有効期限が 1 時間である。
- GCS バケットに `age=1` の Lifecycle Rule が設定されている。

## Decisions / Risks

### Decisions

- Cloud Run は `--allow-unauthenticated` で公開する。
- ジョブの永続化は行わない。
  - Cloud Run の `--max-instances=1` と `--concurrency=1` を初期値にする。
  - 必要に応じて `--no-cpu-throttling` を使う。
- 生成物は GCS に保存し、署名付き URL で配布する。
- 生成物の削除は GCS ライフサイクル（`age=1`）で行う。
- インフラの source-of-truth は Terraform とし、`infra/terraform/cloudrun-public-gcs/` を使う。

### Risks / Mitigations

- 任意 URL を許すと SSRF と濫用のリスクがある。
  - Mitigation: private IP とメタデータを拒否する。
  - Mitigation: 最大ページ数と最大時間を設ける。
  - Mitigation: `max-instances` を小さくする。
- オブジェクト削除が日単位のため、最大 1 日ぶんの保存コストが発生する。
  - Mitigation: 生成物サイズ/生成頻度を監視し、必要なら TTL を短縮する。
- Cloud Run は 1 リクエストの上限がある。
  - Mitigation: タイムアウトと作業量を制限する。

## Progress

- 2026-02-01: `plans/` を復活し ExecPlan を追加した。
- 2026-02-01: Cloud Run + GCS を Terraform で作成できるようにした（GCS lifecycle `age=1`）。

---

## Appendix: アーキテクチャ概要

### コンポーネント

- Cloud Run: `sitebookify`。
  - 公開 Web と API を同梱する。
  - 生成物はローカルではなく GCS に保存する。
- GCS: `sitebookify-artifacts`。
  - zip を保存する。
  - オブジェクトは非公開にする。

### 生成物の TTL

- ダウンロードは署名付き URL を使う。
- 署名付き URL の有効期限を 3600 秒にする。
- オブジェクト削除は GCS ライフサイクルで 1 日削除とする。

---

## Appendix: GCP セットアップ手順

前提条件。

- `gcloud` が使える。
- `terraform` が使える。
- デプロイ先の GCP プロジェクトがある。

### 1. Terraform の認証を行う

```sh
gcloud auth application-default login
```

### 2. Artifact Registry へ push する（例）

Terraform で Artifact Registry は作成されるが、Cloud Run を動かすには
コンテナイメージを push しておく必要がある。

```sh
PROJECT_ID="<your-project-id>"
REGION="<your-region>" # 例: asia-northeast1
AR_REPO="sitebookify"
IMAGE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${AR_REPO}/sitebookify-app:latest"

gcloud auth configure-docker "${REGION}-docker.pkg.dev"
docker build -t "${IMAGE}" .
docker push "${IMAGE}"
```

### 3. Terraform でインフラを作る

```sh
cd infra/terraform/cloudrun-public-gcs
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars

terraform init
terraform apply
```

### 4. 動作確認（smoke）

```sh
cd infra/terraform/cloudrun-public-gcs
URL="$(terraform output -raw cloud_run_service_url)"
curl -fsS "${URL}/healthz"
```

### 5. Lifecycle Rule の確認

```sh
cd infra/terraform/cloudrun-public-gcs
BUCKET="$(terraform output -raw artifact_bucket_name)"
gcloud storage buckets describe "gs://${BUCKET}" --format="yaml(lifecycle)"
```
