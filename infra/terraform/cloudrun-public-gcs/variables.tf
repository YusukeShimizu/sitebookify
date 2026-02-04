variable "project_id" {
  type        = string
  description = "GCP project id."
}

variable "region" {
  type        = string
  description = "GCP region for Cloud Run and Artifact Registry."
}

variable "service_name" {
  type        = string
  default     = "sitebookify"
  description = "Cloud Run service name."
}

variable "container_image" {
  type        = string
  description = "Container image URL to deploy to Cloud Run."
}

variable "openai_api_key" {
  type        = string
  default     = null
  nullable    = true
  sensitive   = true
  description = "Optional (not recommended). OpenAI API key for the app. Passed to Cloud Run as SITEBOOKIFY_OPENAI_API_KEY. Note: secrets passed via Terraform will be stored in terraform state. Prefer `openai_api_key_secret_id`."
}

variable "openai_api_key_secret_id" {
  type        = string
  default     = null
  nullable    = true
  description = "Optional (recommended). Secret Manager secret id for the OpenAI API key (e.g. \"sitebookify-openai-api-key\"). If set, Cloud Run reads it via Secret Manager as SITEBOOKIFY_OPENAI_API_KEY (version=latest)."
}

variable "artifact_registry_repository_id" {
  type        = string
  default     = "sitebookify"
  description = "Artifact Registry repository id (Docker)."
}

variable "runtime_service_account_id" {
  type        = string
  default     = "sitebookify-runtime"
  description = "Service account id for Cloud Run runtime."
}

variable "bucket_name" {
  type        = string
  default     = null
  nullable    = true
  description = "GCS bucket name. Defaults to <project_id>-sitebookify-artifacts."
}

variable "bucket_location" {
  type        = string
  default     = null
  nullable    = true
  description = "GCS bucket location. Defaults to region."
}

variable "bucket_force_destroy" {
  type        = bool
  default     = true
  description = "If true, deleting the bucket also deletes all objects."
}

variable "artifact_lifecycle_delete_age_days" {
  type        = number
  default     = 1
  description = "Delete objects older than this many days."

  validation {
    condition     = var.artifact_lifecycle_delete_age_days >= 1
    error_message = "artifact_lifecycle_delete_age_days must be >= 1."
  }
}

variable "signed_url_ttl_secs" {
  type        = number
  default     = 3600
  description = "Signed URL TTL for downloads."

  validation {
    condition     = var.signed_url_ttl_secs >= 60 && var.signed_url_ttl_secs <= 604800
    error_message = "signed_url_ttl_secs must be between 60 and 604800."
  }
}

variable "concurrency" {
  type        = number
  default     = 1
  description = "Cloud Run concurrency."

  validation {
    condition     = var.concurrency >= 1 && var.concurrency <= 1000
    error_message = "concurrency must be between 1 and 1000."
  }
}

variable "max_instances" {
  type        = number
  default     = 1
  description = "Cloud Run max instances."

  validation {
    condition     = var.max_instances >= 1 && var.max_instances <= 100
    error_message = "max_instances must be between 1 and 100."
  }
}
