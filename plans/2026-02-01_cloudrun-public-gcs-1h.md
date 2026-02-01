# ExecPlan: Cloud Run 公開実行と GCS 1 時間保存

## Goal

- `sitebookify-app` を Cloud Run にデプロイし、一般ユーザがブラウザから実行できるようにする。
- 生成物は GCS に保存し、ダウンロードは 1 時間で失効する URL で提供する。
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
   - 「1 時間保存」の定義を確定する。
2. **GCP リソースを用意する。**
   - Artifact Registry と GCS バケットを作る。
   - Cloud Run の実行用サービスアカウントを作る。
   - 最小権限の IAM を付与する。
3. **Cloud Run にデプロイできるようにする。**
   - 公開アクセスを有効にする。
   - `/healthz` を外部から確認できる。
4. **ダウンロード URL の 1 時間失効を満たす。**
   - 署名付き URL の `expires=3600` を満たす。
5. **GCS の 1 時間削除を満たす。**
   - Cloud Scheduler で定期削除を実行できる。
   - 1 時間を超えた生成物が消える。
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
- 削除ジョブを手動実行し、1 時間超の生成物が消える。

## Decisions / Risks

### Decisions

- Cloud Run は `--allow-unauthenticated` で公開する。
- ジョブの永続化は行わない。
  - Cloud Run の `--max-instances=1` と `--concurrency=1` を初期値にする。
  - 必要に応じて `--no-cpu-throttling` を使う。
- 生成物は GCS に保存し、署名付き URL で配布する。
- 生成物の削除は Cloud Scheduler で定期実行する。
  - 実行先は Cloud Run Jobs を想定する。

### Risks / Mitigations

- 任意 URL を許すと SSRF と濫用のリスクがある。
  - Mitigation: private IP とメタデータを拒否する。
  - Mitigation: 最大ページ数と最大時間を設ける。
  - Mitigation: `max-instances` を小さくする。
- GCS のライフサイクルは日単位であり、1 時間削除を満たさない。
  - Mitigation: 削除は Scheduler で 15 分ごとに走らせる。
  - Mitigation: 保険として GCS ライフサイクルで 1 日削除も入れる。
- Cloud Run は 1 リクエストの上限がある。
  - Mitigation: タイムアウトと作業量を制限する。

## Progress

- 2026-02-01: `plans/` を復活し ExecPlan を追加した。

---

## Appendix: アーキテクチャ概要

### コンポーネント

- Cloud Run: `sitebookify`。
  - 公開 Web と API を同梱する。
  - 生成物はローカルではなく GCS に保存する。
- GCS: `sitebookify-artifacts`。
  - zip を保存する。
  - オブジェクトは非公開にする。
- Cloud Scheduler + Cloud Run Jobs: `sitebookify-cleanup`。
  - 期限超過のオブジェクトを削除する。

### 生成物の TTL

- ダウンロードは署名付き URL を使う。
- 署名付き URL の有効期限を 3600 秒にする。
- オブジェクト削除は 15 分ごとの cleanup で 1 時間を満たす。

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

### 2. Artifact Registry を作る

```sh
PROJECT_ID="<your-project-id>"
REGION="<your-region>"        # 例: asia-northeast1
AR_REPO="sitebookify"

gcloud artifacts repositories create "${AR_REPO}" \
  --project "${PROJECT_ID}" \
  --location "${REGION}" \
  --repository-format docker
```

### 3. GCS バケットを作る

```sh
PROJECT_ID="<your-project-id>"
REGION="<your-region>"  # 例: asia-northeast1
BUCKET="gs://${PROJECT_ID}-sitebookify-artifacts"

gcloud storage buckets create "${BUCKET}" \
  --location "${REGION}" \
  --uniform-bucket-level-access
```

### 4. Cloud Run の実行 SA を作る

```sh
SA_NAME="sitebookify-runtime"
gcloud iam service-accounts create "${SA_NAME}" --project "${PROJECT_ID}"
SA="${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com"
```

### 5. GCS の権限を付ける

```sh
gcloud storage buckets add-iam-policy-binding "${BUCKET}" \
  --member "serviceAccount:${SA}" \
  --role "roles/storage.objectAdmin"
```

### 6. 署名付き URL の署名権限を付ける

鍵ファイルを使わずに署名する場合は `iamcredentials.signBlob` を使う。
そのための権限を付ける。

```sh
gcloud iam service-accounts add-iam-policy-binding "${SA}" \
  --member "serviceAccount:${SA}" \
  --role "roles/iam.serviceAccountTokenCreator"
```

### 7. Cloud Run の pull 権限を確認する

Artifact Registry から pull するには Cloud Run のサービスエージェントに権限が要る。
環境によっては自動付与されない。

```sh
PROJECT_NUMBER="$(gcloud projects describe "${PROJECT_ID}" --format='value(projectNumber)')"
RUN_AGENT="service-${PROJECT_NUMBER}@serverless-robot-prod.iam.gserviceaccount.com"

gcloud artifacts repositories add-iam-policy-binding "${AR_REPO}" \
  --location "${REGION}" \
  --member "serviceAccount:${RUN_AGENT}" \
  --role "roles/artifactregistry.reader"
```

### 8. Cloud Run をデプロイする

この手順は例である。
実際のイメージ名は Artifact Registry に合わせる。

```sh
SERVICE="sitebookify"
IMAGE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${AR_REPO}/sitebookify-app:latest"

gcloud run deploy "${SERVICE}" \
  --project "${PROJECT_ID}" \
  --region "${REGION}" \
  --image "${IMAGE}" \
  --service-account "${SA}" \
  --allow-unauthenticated \
  --concurrency 1 \
  --max-instances 1 \
  --no-cpu-throttling \
  --set-env-vars "SITEBOOKIFY_ARTIFACT_BUCKET=${BUCKET},SITEBOOKIFY_SIGNED_URL_TTL_SECS=3600"
```

### 9. 期限切れ生成物を削除する

GCS ライフサイクルは日単位である。
保険として 1 日削除を入れる。

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

1 時間削除は Cloud Scheduler と Cloud Run Jobs を使う。
この部分はアプリ側の実装が必要である。

