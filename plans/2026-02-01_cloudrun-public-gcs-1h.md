# ExecPlan: Cloud Run 公開実行と GCS 1 時間保存

## Goal

- `sitebookify-app` を Cloud Run にデプロイし、一般ユーザがブラウザから実行できるようにする。
- 生成物は GCS に保存し、ダウンロードは 1 時間で失効する URL で提供する。
- ユーザログインと永続 DB を不要にする。

### Non-goals

- ユーザ認証、課金、利用規約同意などのフローは扱わない。
- ジョブの永続化と履歴表示は扱わない。
- 任意 URL を完全に安全に扱うことは保証しない。

## Scope

### In scope

- GCP リソース設計。
  - Cloud Run（公開 HTTP）。
  - GCS バケット（生成物保管）。
  - IAM とサービスアカウント。
  - 期限切れ生成物の削除方式。
- デプロイ手順と CI/CD 方針。
  - GitHub Actions からのデプロイ方針。
  - 必要な Variables と IAM 権限。
- アプリの設定設計。
  - バケット名、URL 期限、保存プレフィックス。
  - 実行制限（時間、ページ数、並列数）。

### Out of scope

- Firestore などの永続ストア導入。
- Cloud Armor や WAF の本格設定。
- カスタムドメインと TLS 証明書の運用。

## Milestones

1. **アーキテクチャ確定。**
   - 公開 Cloud Run での実行モデルを決める。
   - 出力の「1 時間保存」の定義を決める。
     - 署名付き URL の有効期限を 1 時間にする。
     - オブジェクト削除も 1 時間を目標にする。
2. **GCP リソースを用意する。**
   - GCS バケットを作る。
   - Cloud Run 用のサービスアカウントを作る。
   - 最小権限の IAM を付与する。
3. **Cloud Run にデプロイできるようにする。**
   - 公開アクセスを有効にする。
   - `/healthz` を外部から確認できる。
   - 生成物が GCS に保存できる。
4. **ダウンロードの 1 時間失効を満たす。**
   - 署名付き URL の期限を 3600 秒にする。
   - URL の失効を検証する。
5. **GCS の 1 時間削除を満たす。**
   - GCS のライフサイクルは日単位のため注意する。
   - Cloud Scheduler + Cloud Run Jobs で 1 時間超の生成物を削除する。
6. **CI/CD を整備する。**
   - `main` への push で Cloud Run へデプロイできる。
   - 失敗時にロールバックできる手順を残す。

## Tests

### Local

- `nix develop -c just ci`。
- `docker build -t sitebookify-app:local .`。
- `docker run --rm -p 18080:8080 sitebookify-app:local`。
- `curl -fsS http://127.0.0.1:18080/healthz`。

### Deploy smoke

- Cloud Run の URL に対して `GET /healthz` が `200` になる。
- 既知の小さな URL を入力して生成が完走する。
- GCS に zip が作られ、署名付き URL でダウンロードできる。

### TTL

- 署名付き URL の有効期限が 1 時間である。
- 削除ジョブを手動実行し、1 時間超の生成物が消える。

## Decisions / Risks

### Decisions

- 一般ユーザ向けに Cloud Run を `--allow-unauthenticated` で公開する。
- ジョブは永続化しない。
  - Cloud Run の `--max-instances=1` で単純化する。
  - CPU の常時割り当てでバックグラウンド実行を成立させる。
- 生成物は GCS に置き、署名付き URL で配布する。
- 生成物の削除は Cloud Scheduler で定期実行する。
  - 削除実行は Cloud Run Jobs を使う。
- Cloud Run の上限を小さくしてコストと濫用を抑える。
  - `--max-instances=1` を初期値にする。
  - `--concurrency=1` を初期値にする。

### Risks / Mitigations

- 任意 URL を許すと SSRF と濫用のリスクがある。
  - Mitigation: private IP とメタデータを拒否する。
  - Mitigation: 最大ページ数と最大時間を設ける。
  - Mitigation: `max-instances` を小さくする。
- GCS の削除は時間単位が難しい。
  - Mitigation: 署名付き URL を 1 時間で失効させる。
  - Mitigation: Scheduler で 15 分ごとに削除する。
- Cloud Run は 1 リクエストの上限がある。
  - Mitigation: タイムアウトと作業量を制限する。

## Progress

- 2026-02-01: ExecPlan を作成した。

---

## Appendix: GCP セットアップ手順

前提条件。

- `gcloud` が使える。
- デプロイ先の GCP プロジェクトがある。

### 1. API を有効化する

```sh
gcloud services enable \
  run.googleapis.com \
  artifactregistry.googleapis.com \
  storage.googleapis.com \
  iam.googleapis.com \
  iamcredentials.googleapis.com \
  cloudscheduler.googleapis.com
```

### 2. GCS バケットを作る

```sh
PROJECT_ID="<your-project-id>"
REGION="<your-region>"  # 例: asia-northeast1
BUCKET="gs://${PROJECT_ID}-sitebookify-artifacts"

gcloud storage buckets create "${BUCKET}" \
  --location "${REGION}" \
  --uniform-bucket-level-access
```

### 3. Cloud Run のサービスアカウントを作る

```sh
SA_NAME="sitebookify-runtime"
gcloud iam service-accounts create "${SA_NAME}" --project "${PROJECT_ID}"
SA="${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com"
```

### 4. バケット書き込み権限を付ける

```sh
gcloud storage buckets add-iam-policy-binding "${BUCKET}" \
  --member "serviceAccount:${SA}" \
  --role "roles/storage.objectAdmin"
```

### 5. 署名付き URL の署名権限を付ける

鍵ファイルを使わずに署名する場合は `iamcredentials.signBlob` を使う。
そのための権限を付ける。

```sh
gcloud iam service-accounts add-iam-policy-binding "${SA}" \
  --member "serviceAccount:${SA}" \
  --role "roles/iam.serviceAccountTokenCreator"
```

### 6. Cloud Run をデプロイする

この手順は例である。
実際のイメージ名は Artifact Registry に合わせる。

```sh
SERVICE="sitebookify"
IMAGE="<region>-docker.pkg.dev/<project>/<repo>/sitebookify-app:latest"

gcloud run deploy "${SERVICE}" \
  --project "${PROJECT_ID}" \
  --region "${REGION}" \
  --image "${IMAGE}" \
  --service-account "${SA}" \
  --allow-unauthenticated \
  --no-cpu-throttling \
  --concurrency 1 \
  --max-instances 1
```

### 7. Cloud Run の pull 権限を確認する

Artifact Registry から pull するには Cloud Run のサービスエージェントに権限が要る。
環境によっては自動付与されない。

```sh
PROJECT_NUMBER="$(gcloud projects describe "${PROJECT_ID}" --format='value(projectNumber)')"
RUN_AGENT="service-${PROJECT_NUMBER}@serverless-robot-prod.iam.gserviceaccount.com"

AR_REPO="<your-ar-repo>"
gcloud artifacts repositories add-iam-policy-binding "${AR_REPO}" \
  --location "${REGION}" \
  --member "serviceAccount:${RUN_AGENT}" \
  --role "roles/artifactregistry.reader"
```

### 8. 生成物の削除方式を決める

GCS のライフサイクルは日単位である。
そのため 1 時間削除を厳密にやるなら別方式が要る。

#### 選択肢 A。URL のみ 1 時間で失効させる

- 署名付き URL の `X-Goog-Expires` を 3600 にする。
- オブジェクト削除は日単位に寄せる。

```sh
cat > lifecycle.json <<'JSON'
{
  "rule": [
    {
      "action": { "type": "Delete" },
      "condition": { "age": 1 }
    }
  ]
}
JSON

gcloud storage buckets update "${BUCKET}" --lifecycle-file lifecycle.json
```

#### 選択肢 B。オブジェクトも 1 時間で削除する

- Cloud Scheduler で 15 分ごとに削除処理を走らせる。
- 実行先は Cloud Run Jobs にする。
- 削除条件は作成時刻が 1 時間より古いものにする。

この方式はアプリ側の実装が必要である。
実装は `sitebookify cleanup-gcs` のようなコマンドを想定する。

### 9. GitHub Actions からデプロイする

デプロイ用の Workload Identity Federation を使う。
鍵ファイルは使わない。

必要な GitHub Actions Variables の例を示す。

- `GCP_PROJECT_ID`
- `GCP_REGION`
- `GAR_REPOSITORY`
- `GCP_WORKLOAD_IDENTITY_PROVIDER`
- `GCP_SERVICE_ACCOUNT`
- `CLOUD_RUN_SERVICE`
- `SITEBOOKIFY_ARTIFACT_BUCKET`

デプロイ用のサービスアカウントには権限が要る。
最小権限は環境で変わる。

- `roles/run.admin`
- `roles/iam.serviceAccountUser`（Cloud Run の実行 SA に対して付ける）
- `roles/artifactregistry.reader`
