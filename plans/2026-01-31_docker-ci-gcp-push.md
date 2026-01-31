# ExecPlan: Docker 化 + CI で GCP (Artifact Registry) に Push

## Goal

- `sitebookify-app` をコンテナ化し、GitHub Actions の CI から GCP の Artifact Registry へイメージを push できる状態にする。
- ローカルでも `docker build` / `docker run` で最低限の動作確認（`/healthz`）ができるようにする。

### Non-goals

- Cloud Run / GKE などへの **デプロイ** 自体は扱わない（push まで）。
- LLM エンジン（Codex 等）の実運用の認証/課金/レート制御は扱わない。
- 既存の Nix devShell / `just ci` の仕組みを壊さない（置き換えない）。

## Scope

### In scope

- Docker イメージ作成
  - `sitebookify-app` の Dockerfile（マルチステージで web assets も含める方針を想定）
  - `.dockerignore`
  - ローカル実行の最小ドキュメント（例: `docker run -p 8080:8080 ...`）
- GitHub Actions での build & push
  - Artifact Registry へ push する workflow の追加
  - 認証は Workload Identity Federation（WIF）前提（推奨）
  - タグ戦略（`sha` / `latest` / `tag`）を明文化

### Out of scope

- Helm / Terraform / Pulumi 等の IaC の整備
- 監視（metrics/log routing）や SLO 設計
- コンテナ内の永続ストレージ設計（現状はローカル FS のみ）

## Milestones

1. **現状把握と方針決定**
   - 対象バイナリを `sitebookify-app` に固定（CLI は後回し）
   - Web assets の同梱方針（同梱 or 別ホスティング）を決める
2. **Dockerfile 作成（ローカル build が通る）**
   - `docker build` が成功する
   - `docker run` で `GET /healthz` が `200 ok` になる
3. **CI workflow 追加（push なしの dry-run まで）**
   - PR では build のみ（push しない）で検証できる
   - main / tag では push が走る構成にする（必要 secrets/vars は明記）
4. **GCP 側のセットアップ手順を docs 化**
   - Artifact Registry リポジトリ作成
   - WIF (GitHub OIDC) / Service Account / IAM バインドの手順を記載
5. **実運用に向けた小さな改善（任意）**
   - 生成物の SBOM / provenance（SLSA）や署名（cosign）は拡張ポイントとして残す

### GCP Setup（WIF + Artifact Registry の最小手順）

前提条件。

- `gcloud` が使えること
- GCP 側で対象 project を選べること
- GitHub リポジトリが `<OWNER>/<REPO>` であること（例: `YusukeShimizu/sitebookify`）

#### Artifact Registry

```sh
PROJECT_ID="<your-project-id>"
REGION="<your-region>"        # e.g. asia-northeast1
REPO_NAME="<your-ar-repo>"    # e.g. sitebookify

gcloud services enable artifactregistry.googleapis.com iamcredentials.googleapis.com iam.googleapis.com sts.googleapis.com

gcloud artifacts repositories create "${REPO_NAME}" \
  --repository-format=docker \
  --location="${REGION}"
```

#### Service Account（push 権限）

```sh
SA_NAME="github-actions"

gcloud iam service-accounts create "${SA_NAME}" \
  --project "${PROJECT_ID}"

gcloud artifacts repositories add-iam-policy-binding "${REPO_NAME}" \
  --location "${REGION}" \
  --member "serviceAccount:${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com" \
  --role "roles/artifactregistry.writer"
```

#### Workload Identity Federation（GitHub OIDC）

```sh
OWNER="<github-owner>"
REPO="<github-repo>"

PROJECT_NUMBER="$(gcloud projects describe "${PROJECT_ID}" --format='value(projectNumber)')"

POOL_ID="github"
PROVIDER_ID="github"

gcloud iam workload-identity-pools create "${POOL_ID}" \
  --project="${PROJECT_ID}" \
  --location="global" \
  --display-name="GitHub Actions"

gcloud iam workload-identity-pools providers create-oidc "${PROVIDER_ID}" \
  --project="${PROJECT_ID}" \
  --location="global" \
  --workload-identity-pool="${POOL_ID}" \
  --display-name="GitHub OIDC" \
  --issuer-uri="https://token.actions.githubusercontent.com" \
  --attribute-mapping="google.subject=assertion.sub,attribute.repository=assertion.repository" \
  --attribute-condition="attribute.repository=='${OWNER}/${REPO}'"

gcloud iam service-accounts add-iam-policy-binding "${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com" \
  --project="${PROJECT_ID}" \
  --role="roles/iam.workloadIdentityUser" \
  --member="principalSet://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${POOL_ID}/attribute.repository/${OWNER}/${REPO}"
```

GitHub Actions Variables（workflow 側で参照）。

- `GCP_PROJECT_ID=${PROJECT_ID}`
- `GCP_REGION=${REGION}`
- `GAR_REPOSITORY=${REPO_NAME}`
- `GCP_SERVICE_ACCOUNT=${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com`
- `GCP_WORKLOAD_IDENTITY_PROVIDER`（例: `projects/.../providers/...`）

## Tests

- `just ci`（既存の品質ゲート）
- Docker:
  - `docker build`（`sitebookify-app` イメージがビルドできる）
  - `docker run` + `curl http://127.0.0.1:8080/healthz`（ヘルスチェック）
- CI:
  - PR: build-only が成功（push は行わない）
  - main/tag: 認証情報が入った環境で push が成功（※このリポジトリ外の設定が必要）

## Decisions / Risks

### Decisions

- レジストリは **Artifact Registry**（`<region>-docker.pkg.dev`）を採用する（GCR ではなく）。
- GitHub Actions → GCP の認証は **Workload Identity Federation (OIDC)** を第一候補にする（鍵ファイルを置かない）。
- イメージの entrypoint は `sitebookify-app` とし、コンテナでは `--addr 0.0.0.0:8080` を指定する。
  - （Cloud Run 互換を意識して `8080` をデフォルトに寄せる）

### Risks / Mitigations

- **Rust build が `buf` に依存**しており、Docker build で `buf` を入れる必要がある
  - Mitigation: builder stage で `buf` をインストールし、バージョンを固定する
- `readability-js` が **Rust 2024 の let-chains** を利用しており、古い Rust イメージだとビルドが落ちる
  - Mitigation: Rust builder image は `rust:1.91-bookworm`（`rustc 1.91.1`）以上を使う
- Web build に Node が必要で image build が重くなる
  - Mitigation: web assets 同梱を維持しつつ、レイヤキャッシュが効くように Dockerfile を工夫する
- `web/src/gen` が gitignore 対象のため、クリーン環境では TypeScript build が失敗する
  - Mitigation: Dockerfile / CI で `buf generate` を実行してから `npm run build` する
- GCP 側の IAM/WIF 設定が間違うと push に失敗する
  - Mitigation: 必要な `gcloud` コマンド例と最小権限（`roles/artifactregistry.writer`）を docs に明記する

## Progress

- 2026-01-31: ExecPlan 作成（Docker 化 + CI push の到達点を定義）
- 2026-01-31: `Dockerfile` / `.dockerignore` を追加し、`docker build` と `/healthz` を確認
- 2026-01-31: `.github/workflows/image-gcp.yml` を追加（PR: build-only / main+tag: push）
