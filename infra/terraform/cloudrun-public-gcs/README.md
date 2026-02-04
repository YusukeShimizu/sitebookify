# Terraform: Cloud Run public + GCS artifacts (1 day lifecycle)

This Terraform creates the minimum GCP resources to run `sitebookify-app` on **public Cloud Run**.
Artifacts are stored in a **private GCS bucket** with a **1-day delete lifecycle**.

## Prerequisites

- Terraform installed.
- GCP project is created.
- Auth for Terraform:
  - `gcloud auth application-default login`
  - `gcloud config set project <PROJECT_ID>`
- OpenAI API key (if you use the OpenAI engine):
  - Set `OPENAI_API_KEY` / `SITEBOOKIFY_OPENAI_API_KEY` when running locally.
  - For Cloud Run, prefer Secret Manager:
    - Create a Secret Manager secret (example):
      - `gcloud secrets create sitebookify-openai-api-key --replication-policy=automatic`
      - `printf %s "$OPENAI_API_KEY" | gcloud secrets versions add sitebookify-openai-api-key --data-file=-`
    - Set `openai_api_key_secret_id = "sitebookify-openai-api-key"` in `terraform.tfvars`
    - Terraform grants the runtime service account `roles/secretmanager.secretAccessor`.
  - Quick test (not recommended): set `openai_api_key` in `terraform.tfvars` (stored in terraform state).

## Usage

```sh
cd infra/terraform/cloudrun-public-gcs
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars

terraform init
# (recommended) Create the Artifact Registry repository first, then push the image.
terraform apply -target=google_artifact_registry_repository.sitebookify

# Build & push the image (Dockerfile is at the repo root)
cd "$(git rev-parse --show-toplevel)"
PROJECT_ID="<your-project-id>"
REGION="<your-region>" # e.g. asia-northeast1
AR_REPO="sitebookify"
TAG="git-$(git rev-parse --short HEAD)" # avoid fixed tags like `latest`
IMAGE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${AR_REPO}/sitebookify-app:${TAG}"

gcloud config set project "${PROJECT_ID}"
gcloud auth configure-docker "${REGION}-docker.pkg.dev"
docker buildx build --platform linux/amd64 -t "${IMAGE}" --push .

cd infra/terraform/cloudrun-public-gcs
# Update `terraform.tfvars` (`container_image`) to the pushed `${IMAGE}` before apply.
terraform apply
```

Destroy:

```sh
terraform destroy
```

## Notes

- Artifact deletion is implemented by **GCS lifecycle**: `age = 1` (days).
- Cloud Run is made public by granting `roles/run.invoker` to `allUsers`.
