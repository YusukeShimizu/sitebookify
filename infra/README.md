# Infra (GCP)

このディレクトリは `sitebookify-app` を **GCP** 上で動かすための IaC（Terraform）を置く。

- Terraform: `infra/terraform/`
- ローカル実行: `sitebookify-app` は生成物（zip）をローカル FS（`--data-dir`）に作って `/artifacts/:job_id` で配信する。
- Cloud Run（`infra/terraform/cloudrun-public-gcs/`）:
  - 生成物は **GCS に保存**する。
  - ダウンロードは **署名付き URL** を返す（デフォルト TTL: 3600 秒）。
  - オブジェクト削除は GCS ライフサイクルで `age = 1`（日）としている。

## 先に GCP 側で用意するもの（必須）

1) **GCP Project**
- プロジェクト作成（`project_id` を決める）
- Billing 有効化（Cloud Run / Artifact Registry / GCS を使うため）

2) **Terraform を実行できる認証**
- 手元実行（推奨・最短）: Application Default Credentials を使う
  - `gcloud auth application-default login`
    - もし `UNAUTHENTICATED ... auth/disable_credentials ...` のようなエラーが出る。
      `auth/disable_credentials` が有効な可能性がある。
      - まず `gcloud config unset auth/disable_credentials` を実行する。
      - 直らなければ `gcloud config set auth/disable_credentials false` を実行する。
      - 次に `gcloud auth application-default login` を実行する。
  - `gcloud config set project <PROJECT_ID>`（`PROJECT_ID` は project id。プロジェクト名（表示名）ではない）
  - （推奨）ADC の quota project を揃える: `gcloud auth application-default set-quota-project <PROJECT_ID>`
    - 権限不足（`serviceusage.services.use`）で失敗する場合は、そのプロジェクトに対する権限付与が必要
- もしくは CI 用の Service Account を作って `GOOGLE_APPLICATION_CREDENTIALS` で渡す

3) **コンテナイメージ（Artifact Registry へ push できる状態）**
- Cloud Run のデプロイには `container_image` が必要。
- `infra/terraform/cloudrun-public-gcs/` は Artifact Registry リポジトリまで作る。
  - **イメージの push は別途**（ローカルまたは GitHub Actions）。

## Terraform が作るもの（`cloudrun-public-gcs`）

Terraform: `infra/terraform/cloudrun-public-gcs/`

- API 有効化:
  - `run.googleapis.com`
  - `storage.googleapis.com`
  - `artifactregistry.googleapis.com`
  - `iam.googleapis.com`
  - `iamcredentials.googleapis.com`
  - `secretmanager.googleapis.com`
- Artifact Registry (Docker) リポジトリ
- Cloud Run（`allUsers` に `roles/run.invoker` 付与 = 公開）
- Cloud Run 実行用 Service Account（最小権限寄せ）
- GCS バケット（生成物の保管先。非公開。Lifecycle で一定日数後に削除）

変えたい値は `infra/terraform/cloudrun-public-gcs/variables.tf` を参照。

## 手順（ローカルから Terraform apply）

### 0) 変数を用意

```sh
cd infra/terraform/cloudrun-public-gcs
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars
```

`container_image` は次の形式で指定する。

```text
<REGION>-docker.pkg.dev/<PROJECT_ID>/<REPO>/sitebookify-app:<TAG>
```

Terraform で Cloud Run を管理する場合、固定 tag（例: `latest`）のまま push すると差分検知できないことがある。
Revision は digest 固定のため、`terraform apply` でも更新されないことがある。
そのため **tag を毎回変える（推奨）** か、digest（`@sha256:...`）指定にする。

OpenAI エンジンを使う場合は `openai_api_key_secret_id`（推奨）または `openai_api_key` を設定する。
詳細は `infra/terraform/cloudrun-public-gcs/README.md` を参照。

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
cd "$(git rev-parse --show-toplevel)" # repo root (Dockerfile is here)

PROJECT_ID="<your-project-id>"
REGION="<your-region>" # 例: asia-northeast1
AR_REPO="sitebookify"
TAG="git-$(git rev-parse --short HEAD)" # 例: git-a1b2c3d（固定 tag を避ける）
IMAGE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${AR_REPO}/sitebookify-app:${TAG}"

gcloud config set project "${PROJECT_ID}"
gcloud auth configure-docker "${REGION}-docker.pkg.dev"

# Apple Silicon などで build が失敗する場合は linux/amd64 を指定する（Dockerfile が x86_64 の buf を使うため）
docker buildx build --platform linux/amd64 -t "${IMAGE}" --push .
# もしくは（x86_64 環境など）:
# docker build -t "${IMAGE}" .
# docker push "${IMAGE}"
```

### 2) Terraform apply

`terraform.tfvars` の `container_image` も、今 push した `${IMAGE}` に更新してから apply する。

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
  - `GCP_WORKLOAD_IDENTITY_PROVIDER`: 形式は次の通り。
    ```text
    projects/<number>/locations/global/workloadIdentityPools/<pool>/providers/<provider>
    ```
  - `GCP_SERVICE_ACCOUNT`: `github-actions@<project>.iam.gserviceaccount.com`

GCP 側の具体手順は構成差が大きいので、まずはリポジトリの `README.md` の
「CI: GCP Artifact Registry へ Docker image を push」を参照。

### いつ push されるか

`image-gcp` workflow はイベントで挙動が分かれる。

- `pull_request`: build のみ（push しない）
- `main` への push / `v*` tag: build + push（上記 Variables が揃っている場合）

## GitHub Actions で Cloud Run へ deploy（任意）

`main` への push 時に Cloud Run を更新したい場合は次を使う。

- Workflow: `.github/workflows/deploy-cloudrun.yml`
- トリガー: `Image (GCP Artifact Registry)` workflow 完了（`main` push のみ）
- 追加 Variables:
  - `CLOUD_RUN_SERVICE`（Terraform の `service_name` と揃える。デフォルトは `sitebookify`）

`GCP_SERVICE_ACCOUNT` には権限が必要。  
`roles/run.admin` と `roles/iam.serviceAccountUser` を付与する（例）。

## 運用メモ（最小）

- 公開 Cloud Run は濫用されやすいので、必要なら `max_instances` を絞る・認証を付ける・WAF/Cloud Armor を検討する。
- Terraform state は現状ローカル（`terraform.tfstate`）で管理する。チーム運用するなら GCS backend を用意するのがおすすめ。
