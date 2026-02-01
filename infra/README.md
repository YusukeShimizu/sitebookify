# Infra (GCP)

このディレクトリは `sitebookify-app` を **GCP** 上で動かすための IaC（Terraform）を置く。

- Terraform: `infra/terraform/`
- 現状の実装: `sitebookify-app` は生成物（zip）をローカル FS（`--data-dir`）に作って `/artifacts/:job_id` で配信する。
  - `infra/terraform/cloudrun-public-gcs/` は **GCS 保存＋署名付き URL** を見据えたリソース（バケット/権限/環境変数）も作る（アプリ側対応は別途）。

## 先に GCP 側で用意するもの（必須）

1) **GCP Project**
- プロジェクト作成（`project_id` を決める）
- Billing 有効化（Cloud Run / Artifact Registry / GCS を使うため）

2) **Terraform を実行できる認証**
- 手元実行（推奨・最短）: Application Default Credentials を使う
  - `gcloud auth application-default login`
  - `gcloud config set project <PROJECT_ID>`
- もしくは CI 用の Service Account を作って `GOOGLE_APPLICATION_CREDENTIALS` で渡す

3) **コンテナイメージ（Artifact Registry へ push できる状態）**
- Cloud Run のデプロイには `container_image` が必要。
- `infra/terraform/cloudrun-public-gcs/` は Artifact Registry リポジトリを作るが、**イメージの push は別途**（ローカル or GitHub Actions）。

## Terraform が作るもの（`cloudrun-public-gcs`）

Terraform: `infra/terraform/cloudrun-public-gcs/`

- API 有効化: `run.googleapis.com`, `storage.googleapis.com`, `artifactregistry.googleapis.com`, `iam.googleapis.com`, `iamcredentials.googleapis.com`
- Artifact Registry (Docker) リポジトリ
- Cloud Run（`allUsers` に `roles/run.invoker` 付与 = 公開）
- Cloud Run 実行用 Service Account（最小権限寄せ）
- GCS バケット（生成物保管想定、非公開、Lifecycle で一定日数後に削除）

変えたい値は `infra/terraform/cloudrun-public-gcs/variables.tf` を参照。

## 手順（ローカルから Terraform apply）

### 0) 変数を用意

```sh
cd infra/terraform/cloudrun-public-gcs
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars
```

`container_image` は次の形式（例）:

```text
<REGION>-docker.pkg.dev/<PROJECT_ID>/<REPO>/sitebookify-app:latest
```

### 1) コンテナを build & push（例: ローカル）

> Artifact Registry リポジトリ（`GAR_REPOSITORY` / `artifact_registry_repository_id`）は Terraform が作る。
> 初回は先にリポジトリだけ作ってから push するのが安全。
>
> ```sh
> cd infra/terraform/cloudrun-public-gcs
> terraform init
> terraform apply -target=google_artifact_registry_repository.sitebookify
> ```

```sh
PROJECT_ID="<your-project-id>"
REGION="<your-region>" # 例: asia-northeast1
AR_REPO="sitebookify"
IMAGE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${AR_REPO}/sitebookify-app:latest"

gcloud auth configure-docker "${REGION}-docker.pkg.dev"
docker build -t "${IMAGE}" .
docker push "${IMAGE}"
```

### 2) Terraform apply

```sh
cd infra/terraform/cloudrun-public-gcs
terraform init
terraform apply
```

### 3) 動作確認（smoke）

```sh
cd infra/terraform/cloudrun-public-gcs
URL="$(terraform output -raw cloud_run_service_url)"
curl -fsS "${URL}/healthz"
```

### 4) 後片付け

```sh
cd infra/terraform/cloudrun-public-gcs
terraform destroy
```

## GitHub Actions でイメージを push（任意）

Artifact Registry への push は GitHub Actions でも可能（鍵ファイル不要）。

- Workflow: `.github/workflows/image-gcp.yml`
- 認証: GitHub OIDC → GCP Workload Identity Federation
- 設定場所: GitHub → Repository → `Settings` → `Secrets and variables` → `Actions` → `Variables`
- GitHub Actions Variables（例）:
  - `GCP_PROJECT_ID`: `your-gcp-project-id`
  - `GCP_REGION`: `asia-northeast1`
  - `GAR_REPOSITORY`: `sitebookify`（Terraform の `artifact_registry_repository_id` と揃える）
  - `GCP_WORKLOAD_IDENTITY_PROVIDER`: `projects/<number>/locations/global/workloadIdentityPools/<pool>/providers/<provider>`
  - `GCP_SERVICE_ACCOUNT`: `github-actions@<project>.iam.gserviceaccount.com`

GCP 側の具体手順は構成差が大きいので、まずはリポジトリの `README.md` の
「CI: GCP Artifact Registry へ Docker image を push」を参照。

### いつ push される？

`image-gcp` workflow はイベントで挙動が分かれる。

- `pull_request`: build のみ（push しない）
- `main` への push / `v*` tag: build + push（上記 Variables が揃っている場合）

## 運用メモ（最小）

- 公開 Cloud Run は濫用されやすいので、必要なら `max_instances` を絞る・認証を付ける・WAF/Cloud Armor を検討する。
- Terraform state は今はローカル（`terraform.tfstate`）になる。チーム運用するなら GCS backend を用意するのがおすすめ。
