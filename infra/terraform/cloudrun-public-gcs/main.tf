data "google_project" "current" {
  project_id = var.project_id
}

locals {
  bucket_name     = coalesce(var.bucket_name, "${var.project_id}-sitebookify-artifacts")
  bucket_location = coalesce(var.bucket_location, var.region)

  cloud_run_service_agent = "service-${data.google_project.current.number}@serverless-robot-prod.iam.gserviceaccount.com"

  openai_api_key           = var.openai_api_key == null ? null : trimspace(var.openai_api_key)
  openai_api_key_secret_id = var.openai_api_key_secret_id == null ? null : trimspace(var.openai_api_key_secret_id)
  execution_mode           = lower(trimspace(var.execution_mode))
  worker_auth_token        = var.worker_auth_token == null ? null : trimspace(var.worker_auth_token)
}

resource "google_project_service" "required" {
  for_each = toset([
    "artifactregistry.googleapis.com",
    "iam.googleapis.com",
    "iamcredentials.googleapis.com",
    "run.googleapis.com",
    "secretmanager.googleapis.com",
    "storage.googleapis.com",
  ])

  project            = var.project_id
  service            = each.value
  disable_on_destroy = false
}

resource "google_artifact_registry_repository" "sitebookify" {
  project       = var.project_id
  location      = var.region
  repository_id = var.artifact_registry_repository_id
  format        = "DOCKER"

  depends_on = [google_project_service.required]
}

resource "google_artifact_registry_repository_iam_member" "cloud_run_service_agent_reader" {
  project    = google_artifact_registry_repository.sitebookify.project
  location   = google_artifact_registry_repository.sitebookify.location
  repository = google_artifact_registry_repository.sitebookify.name
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${local.cloud_run_service_agent}"
}

resource "google_service_account" "runtime" {
  project      = var.project_id
  account_id   = var.runtime_service_account_id
  display_name = "sitebookify Cloud Run runtime"

  depends_on = [google_project_service.required]
}

resource "google_service_account_iam_member" "runtime_token_creator_self" {
  service_account_id = google_service_account.runtime.name
  role               = "roles/iam.serviceAccountTokenCreator"
  member             = "serviceAccount:${google_service_account.runtime.email}"
}

resource "google_secret_manager_secret_iam_member" "runtime_openai_key_accessor" {
  count = local.openai_api_key_secret_id == null || local.openai_api_key_secret_id == "" ? 0 : 1

  project   = var.project_id
  secret_id = local.openai_api_key_secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.runtime.email}"
}

resource "google_storage_bucket" "artifacts" {
  name                        = local.bucket_name
  location                    = local.bucket_location
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = var.bucket_force_destroy

  lifecycle_rule {
    action {
      type = "Delete"
    }
    condition {
      age = var.artifact_lifecycle_delete_age_days
    }
  }

  depends_on = [google_project_service.required]
}

resource "google_storage_bucket_iam_member" "runtime_object_admin" {
  bucket = google_storage_bucket.artifacts.name
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.runtime.email}"
}

resource "google_cloud_run_v2_service" "sitebookify" {
  name     = var.service_name
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  template {
    service_account = google_service_account.runtime.email

    scaling {
      max_instance_count = var.max_instances
    }

    max_instance_request_concurrency = var.concurrency

    containers {
      image = var.container_image

      ports {
        container_port = 8080
      }

      env {
        name  = "SITEBOOKIFY_ARTIFACT_BUCKET"
        value = google_storage_bucket.artifacts.name
      }

      env {
        name  = "SITEBOOKIFY_SIGNED_URL_TTL_SECS"
        value = tostring(var.signed_url_ttl_secs)
      }

      env {
        name  = "SITEBOOKIFY_EXECUTION_MODE"
        value = local.execution_mode
      }

      env {
        name  = "SITEBOOKIFY_WORKER_URL"
        value = google_cloud_run_v2_service.sitebookify_worker.uri
      }

      dynamic "env" {
        for_each = local.worker_auth_token == null || local.worker_auth_token == "" ? [] : [local.worker_auth_token]
        content {
          name  = "SITEBOOKIFY_WORKER_AUTH_TOKEN"
          value = env.value
        }
      }

      dynamic "env" {
        for_each = local.openai_api_key_secret_id == null || local.openai_api_key_secret_id == "" ? [] : [local.openai_api_key_secret_id]
        content {
          name = "SITEBOOKIFY_OPENAI_API_KEY"
          value_source {
            secret_key_ref {
              secret  = env.value
              version = "latest"
            }
          }
        }
      }

      dynamic "env" {
        for_each = (local.openai_api_key_secret_id != null && local.openai_api_key_secret_id != "") || local.openai_api_key == null || local.openai_api_key == "" ? [] : [local.openai_api_key]
        content {
          name  = "SITEBOOKIFY_OPENAI_API_KEY"
          value = env.value
        }
      }
    }
  }

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  depends_on = [
    google_project_service.required,
    google_artifact_registry_repository_iam_member.cloud_run_service_agent_reader,
    google_secret_manager_secret_iam_member.runtime_openai_key_accessor,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "public_invoker" {
  location = google_cloud_run_v2_service.sitebookify.location
  name     = google_cloud_run_v2_service.sitebookify.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}

resource "google_cloud_run_v2_service" "sitebookify_worker" {
  name     = var.worker_service_name
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  template {
    service_account = google_service_account.runtime.email

    scaling {
      max_instance_count = var.max_instances
    }

    max_instance_request_concurrency = 1

    containers {
      image = var.container_image

      ports {
        container_port = 8080
      }

      env {
        name  = "SITEBOOKIFY_ARTIFACT_BUCKET"
        value = google_storage_bucket.artifacts.name
      }

      env {
        name  = "SITEBOOKIFY_SIGNED_URL_TTL_SECS"
        value = tostring(var.signed_url_ttl_secs)
      }

      env {
        name  = "SITEBOOKIFY_EXECUTION_MODE"
        value = "inprocess"
      }

      dynamic "env" {
        for_each = local.worker_auth_token == null || local.worker_auth_token == "" ? [] : [local.worker_auth_token]
        content {
          name  = "SITEBOOKIFY_INTERNAL_DISPATCH_TOKEN"
          value = env.value
        }
      }

      dynamic "env" {
        for_each = local.openai_api_key_secret_id == null || local.openai_api_key_secret_id == "" ? [] : [local.openai_api_key_secret_id]
        content {
          name = "SITEBOOKIFY_OPENAI_API_KEY"
          value_source {
            secret_key_ref {
              secret  = env.value
              version = "latest"
            }
          }
        }
      }

      dynamic "env" {
        for_each = (local.openai_api_key_secret_id != null && local.openai_api_key_secret_id != "") || local.openai_api_key == null || local.openai_api_key == "" ? [] : [local.openai_api_key]
        content {
          name  = "SITEBOOKIFY_OPENAI_API_KEY"
          value = env.value
        }
      }
    }
  }

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  depends_on = [
    google_project_service.required,
    google_artifact_registry_repository_iam_member.cloud_run_service_agent_reader,
    google_secret_manager_secret_iam_member.runtime_openai_key_accessor,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "worker_public_invoker" {
  location = google_cloud_run_v2_service.sitebookify_worker.location
  name     = google_cloud_run_v2_service.sitebookify_worker.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}
