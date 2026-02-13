output "artifact_bucket_name" {
  value = google_storage_bucket.artifacts.name
}

output "artifact_registry_repository" {
  value = google_artifact_registry_repository.sitebookify.name
}

output "cloud_run_service_name" {
  value = google_cloud_run_v2_service.sitebookify.name
}

output "cloud_run_service_url" {
  value = google_cloud_run_v2_service.sitebookify.uri
}

output "cloud_run_worker_service_name" {
  value = google_cloud_run_v2_service.sitebookify_worker.name
}

output "cloud_run_worker_service_url" {
  value = google_cloud_run_v2_service.sitebookify_worker.uri
}

output "runtime_service_account_email" {
  value = google_service_account.runtime.email
}
