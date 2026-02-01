# Terraform: Cloud Run public + GCS artifacts (1 day lifecycle)

This Terraform creates the minimum GCP resources to run `sitebookify-app` on **public Cloud Run**.
Artifacts are stored in a **private GCS bucket** with a **1-day delete lifecycle**.

## Prerequisites

- Terraform installed.
- GCP project is created.
- Auth for Terraform:
  - `gcloud auth application-default login`
  - `gcloud config set project <PROJECT_ID>`

## Usage

```sh
cd infra/terraform/cloudrun-public-gcs
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars

terraform init
terraform apply
```

Destroy:

```sh
terraform destroy
```

## Notes

- Artifact deletion is implemented by **GCS lifecycle**: `age = 1` (days).
- Cloud Run is made public by granting `roles/run.invoker` to `allUsers`.
