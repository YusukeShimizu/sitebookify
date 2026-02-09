use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::error_handling::HandleErrorLayer;
use axum::extract::{Path, Query, State};
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Json;
use axum::response::{Html, Response};
use axum::routing::get;
use clap::Parser;
use http_body_util::BodyExt as _;
use serde::Deserialize;
use tokio_util::io::ReaderStream;
use tonic::{Request, Response as TonicResponse, Status};
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use sitebookify::app::artifact_store::{ArtifactStore, GcsArtifactStore, LocalFsArtifactStore};
use sitebookify::app::job_store::{JobStore, LocalFsJobStore};
use sitebookify::app::model::{Job, JobStatus, StartJobRequest};
use sitebookify::app::queue::InProcessQueue;
use sitebookify::app::runner::{JobRunner, default_job_work_dir};
use sitebookify::cli::LlmEngine;
use sitebookify::google::longrunning::operations_server::{
    Operations as LongrunningOperations, OperationsServer as LongrunningOperationsServer,
};
use sitebookify::google::longrunning::{
    CancelOperationRequest, DeleteOperationRequest, GetOperationRequest, ListOperationsRequest,
    ListOperationsResponse, Operation,
};
use sitebookify::google::rpc::Status as RpcStatus;
use sitebookify::grpc::v1::job::State as PbJobState;
use sitebookify::grpc::v1::sitebookify_service_server::{
    SitebookifyService, SitebookifyServiceServer,
};
use sitebookify::grpc::v1::{
    CreateJobMetadata, CreateJobRequest, Engine, GenerateJobDownloadUrlRequest,
    GenerateJobDownloadUrlResponse, GetJobRequest, Job as PbJob, JobSpec, ListJobsRequest,
    ListJobsResponse,
};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct AppArgs {
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: SocketAddr,

    #[arg(long, default_value = "workspace-app")]
    data_dir: PathBuf,

    #[arg(long, default_value_t = 1)]
    max_concurrency: usize,

    /// Static web assets directory (serve if exists).
    #[arg(long, default_value = "web/dist")]
    web_dir: PathBuf,
}

#[derive(Clone)]
struct AppState {
    base_dir: PathBuf,
    job_store: Arc<dyn JobStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    signed_url_ttl_secs: u32,
    queue: InProcessQueue,
    runner: Arc<JobRunner>,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    if let Err(err) = try_main().await {
        eprintln!("{err:#}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

async fn try_main() -> anyhow::Result<()> {
    sitebookify::logging::init()?;

    let args = AppArgs::parse();
    tracing::info!(?args, "starting sitebookify-app");

    let job_store: Arc<dyn JobStore> = Arc::new(LocalFsJobStore::new(&args.data_dir));
    let artifact_bucket = std::env::var("SITEBOOKIFY_ARTIFACT_BUCKET")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let signed_url_ttl_secs = std::env::var("SITEBOOKIFY_SIGNED_URL_TTL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|v| *v >= 60 && *v <= 604_800)
        .unwrap_or(3600);

    let artifact_store: Arc<dyn ArtifactStore> = match &artifact_bucket {
        Some(bucket) => {
            tracing::info!(bucket = %bucket, signed_url_ttl_secs, "using GCS artifact store");
            Arc::new(GcsArtifactStore::new(args.data_dir.clone(), bucket.clone()))
        }
        None => {
            tracing::info!(signed_url_ttl_secs, "using local filesystem artifact store");
            Arc::new(LocalFsArtifactStore::new(args.data_dir.clone()))
        }
    };
    let runner = Arc::new(JobRunner::new(
        Arc::clone(&job_store),
        Arc::clone(&artifact_store),
    ));
    let state = AppState {
        base_dir: args.data_dir,
        job_store,
        artifact_store,
        signed_url_ttl_secs,
        queue: InProcessQueue::new(args.max_concurrency),
        runner,
    };

    let grpc_impl = GrpcSitebookifyService {
        state: state.clone(),
    };
    let grpc_service = tonic::transport::Server::builder()
        .accept_http1(true)
        .add_service(tonic_web::enable(SitebookifyServiceServer::new(grpc_impl)))
        .into_service();
    let grpc_service = ServiceBuilder::new()
        .map_request(|req: axum::http::Request<axum::body::Body>| {
            let (parts, body) = req.into_parts();
            let body = body
                .map_err(|err| Status::internal(err.to_string()))
                .boxed_unsync();
            axum::http::Request::from_parts(parts, body)
        })
        .layer(HandleErrorLayer::new(
            |err: Box<dyn std::error::Error + Send + Sync>| async move {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("gRPC error: {err}"),
                )
            },
        ))
        .service(grpc_service);

    let ops_impl = GrpcOperations {
        state: state.clone(),
    };
    let ops_service = tonic::transport::Server::builder()
        .accept_http1(true)
        .add_service(tonic_web::enable(LongrunningOperationsServer::new(
            ops_impl,
        )))
        .into_service();
    let ops_service = ServiceBuilder::new()
        .map_request(|req: axum::http::Request<axum::body::Body>| {
            let (parts, body) = req.into_parts();
            let body = body
                .map_err(|err| Status::internal(err.to_string()))
                .boxed_unsync();
            axum::http::Request::from_parts(parts, body)
        })
        .layer(HandleErrorLayer::new(
            |err: Box<dyn std::error::Error + Send + Sync>| async move {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("gRPC error: {err}"),
                )
            },
        ))
        .service(ops_service);

    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/preview", get(preview_site_handler))
        .route("/artifacts/:job_id", get(download_artifact))
        .route("/jobs/:job_id/book.md", get(download_book_md))
        .route("/jobs/:job_id/book.epub", get(download_book_epub))
        .route_service("/sitebookify.v1.SitebookifyService/*rest", grpc_service)
        .route_service("/google.longrunning.Operations/*rest", ops_service)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let web_index = args.web_dir.join("index.html");
    if web_index.exists() {
        let static_files = ServeDir::new(args.web_dir).not_found_service(ServeFile::new(web_index));
        app = app.fallback_service(static_files);
    } else {
        app = app.fallback(|| async {
            Html(
                r#"<!doctype html>
<html>
  <head><meta charset="utf-8"><title>sitebookify-app</title></head>
  <body>
    <h1>sitebookify-app</h1>
    <p>web assets not found. Build the web app into <code>web/dist</code> or run a dev server.</p>
  </body>
</html>
"#,
            )
        });
    }

    let listener = tokio::net::TcpListener::bind(args.addr)
        .await
        .map_err(|err| anyhow::anyhow!("bind {}: {err}", args.addr))?;
    tracing::info!(addr = %args.addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn download_artifact(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Response, axum::http::StatusCode> {
    if uuid::Uuid::parse_str(job_id.trim()).is_err() {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    let Some(job) = state
        .job_store
        .get(&job_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(axum::http::StatusCode::NOT_FOUND);
    };

    if job.status != JobStatus::Done {
        return Err(axum::http::StatusCode::CONFLICT);
    }

    if job
        .artifact_uri
        .as_deref()
        .is_some_and(|uri| uri.starts_with("gs://"))
    {
        let url = state
            .artifact_store
            .generate_download_url(&job_id, state.signed_url_ttl_secs)
            .await
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

        let mut resp = Response::new(axum::body::Body::empty());
        *resp.status_mut() = axum::http::StatusCode::TEMPORARY_REDIRECT;
        resp.headers_mut().insert(
            header::LOCATION,
            HeaderValue::from_str(&url)
                .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        return Ok(resp);
    }

    let Some(path) = job.artifact_path else {
        return Err(axum::http::StatusCode::NOT_FOUND);
    };

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;
    let stream = ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"sitebookify-{job_id}.zip\""
        ))
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok(resp)
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    url: String,
}

async fn preview_site_handler(
    Query(q): Query<PreviewQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let raw = q.url.trim();
    if raw.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "url is required".to_string()));
    }

    let url = url::Url::parse(raw).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid url query parameter: {err}"),
        )
    })?;
    let url = sitebookify::crawl::resolve_start_url_for_crawl(&url).await;

    let preview = sitebookify::app::preview::preview_site(&url)
        .await
        .map_err(|err| (StatusCode::BAD_GATEWAY, format!("preview failed: {err:#}")))?;
    Ok(Json(preview))
}

async fn download_book_md(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Response, axum::http::StatusCode> {
    if uuid::Uuid::parse_str(job_id.trim()).is_err() {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    let Some(job) = state
        .job_store
        .get(&job_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(axum::http::StatusCode::NOT_FOUND);
    };

    if job.status != JobStatus::Done {
        return Err(axum::http::StatusCode::CONFLICT);
    }

    let path = job.work_dir.join("book.md");
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;
    let stream = ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline; filename=\"book.md\""),
    );
    Ok(resp)
}

async fn download_book_epub(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Response, axum::http::StatusCode> {
    if uuid::Uuid::parse_str(job_id.trim()).is_err() {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    let Some(job) = state
        .job_store
        .get(&job_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(axum::http::StatusCode::NOT_FOUND);
    };

    if job.status != JobStatus::Done {
        return Err(axum::http::StatusCode::CONFLICT);
    }

    let path = job.work_dir.join("book.epub");
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;
    let stream = ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/epub+zip"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"sitebookify-{job_id}.epub\""
        ))
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok(resp)
}

#[derive(Clone)]
struct GrpcSitebookifyService {
    state: AppState,
}

#[tonic::async_trait]
impl SitebookifyService for GrpcSitebookifyService {
    async fn create_job(
        &self,
        request: Request<CreateJobRequest>,
    ) -> Result<TonicResponse<Operation>, Status> {
        let req = request.into_inner();

        let Some(job) = req.job else {
            return Err(Status::invalid_argument("job is required"));
        };
        let Some(spec) = job.spec else {
            return Err(Status::invalid_argument("job.spec is required"));
        };

        let job_id = if req.job_id.trim().is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            let job_id = req.job_id.trim();
            uuid::Uuid::parse_str(job_id)
                .map_err(|err| Status::invalid_argument(format!("invalid job_id: {err}")))?;
            job_id.to_string()
        };

        if spec.source_url.trim().is_empty() {
            return Err(Status::invalid_argument("job.spec.source_url is required"));
        }
        let url = url::Url::parse(spec.source_url.trim()).map_err(|err| {
            Status::invalid_argument(format!("invalid job.spec.source_url: {err}"))
        })?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(Status::invalid_argument(
                "job.spec.source_url must be http/https",
            ));
        }
        let url = sitebookify::crawl::resolve_start_url_for_crawl(&url).await;

        let work_dir = default_job_work_dir(&self.state.base_dir, &job_id);

        let delay_ms = match spec.request_delay {
            None => StartJobRequest::default_delay_ms(),
            Some(delay) => duration_to_ms(&delay).map_err(Status::invalid_argument)?,
        };

        let start_request = StartJobRequest {
            url: url.to_string(),
            title: spec.title.trim().to_string().into_option(),
            max_pages: i32_as_usize_or_default(
                spec.max_pages,
                StartJobRequest::default_max_pages(),
            )
            .map_err(Status::invalid_argument)?,
            max_depth: i32_as_u32_or_default(spec.max_depth, StartJobRequest::default_max_depth())
                .map_err(Status::invalid_argument)?,
            concurrency: i32_as_usize_or_default(
                spec.concurrency,
                StartJobRequest::default_concurrency(),
            )
            .map_err(Status::invalid_argument)?,
            delay_ms,
            language: string_or_default(spec.language_code, StartJobRequest::default_language()),
            tone: string_or_default(spec.tone, StartJobRequest::default_tone()),
            toc_engine: engine_or_default(spec.toc_engine, StartJobRequest::default_engine())
                .map_err(Status::invalid_argument)?,
            render_engine: engine_or_default(spec.render_engine, StartJobRequest::default_engine())
                .map_err(Status::invalid_argument)?,
        };

        let job = Job {
            job_id: job_id.clone(),
            status: JobStatus::Queued,
            progress_percent: 0,
            message: "queued".to_string(),
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            work_dir,
            artifact_path: None,
            artifact_uri: None,
        };

        self.state
            .job_store
            .create(&job, &start_request)
            .await
            .map_err(|err| Status::internal(format!("create job: {err:#}")))?;

        let runner = Arc::clone(&self.state.runner);
        let job_id_for_task = job_id.clone();
        self.state.queue.spawn(async move {
            runner.run_job(&job_id_for_task).await;
        });

        let now = chrono::Utc::now();
        let metadata = CreateJobMetadata {
            job: job_name(&job_id),
            create_time: Some(timestamp_from_chrono(now)),
            start_time: None,
            completion_time: None,
            progress_percent: 0,
            message: "queued".to_string(),
        };

        let op = Operation {
            name: operation_name(&job_id),
            metadata: Some(pack_any(
                "type.googleapis.com/sitebookify.v1.CreateJobMetadata",
                &metadata,
            )),
            done: false,
            result: None,
        };

        Ok(TonicResponse::new(op))
    }

    async fn get_job(
        &self,
        request: Request<GetJobRequest>,
    ) -> Result<TonicResponse<PbJob>, Status> {
        let job_id =
            job_id_from_name(&request.into_inner().name).map_err(Status::invalid_argument)?;
        let Some(job) = self
            .state
            .job_store
            .get(&job_id)
            .await
            .map_err(|err| Status::internal(format!("get job: {err:#}")))?
        else {
            return Err(Status::not_found("job not found"));
        };

        let Some(start_request) = self
            .state
            .job_store
            .get_request(&job_id)
            .await
            .map_err(|err| Status::internal(format!("get job request: {err:#}")))?
        else {
            return Err(Status::internal("job request not found"));
        };

        Ok(TonicResponse::new(job_to_pb(&job, &start_request)))
    }

    async fn list_jobs(
        &self,
        request: Request<ListJobsRequest>,
    ) -> Result<TonicResponse<ListJobsResponse>, Status> {
        let req = request.into_inner();
        if !req.filter.trim().is_empty() || !req.order_by.trim().is_empty() {
            tracing::warn!(
                filter = req.filter,
                order_by = req.order_by,
                "ListJobs filter/order_by are ignored in the local implementation"
            );
        }

        let mut job_ids = list_local_job_ids(&self.state.base_dir)
            .await
            .map_err(|err| Status::internal(format!("list jobs: {err:#}")))?;
        job_ids.sort();

        let page_size = if req.page_size <= 0 {
            100
        } else {
            req.page_size as usize
        };
        let start_index = if req.page_token.trim().is_empty() {
            0
        } else {
            let token = req.page_token.trim();
            let pos = job_ids
                .iter()
                .position(|id| id == token)
                .ok_or_else(|| Status::invalid_argument("invalid page_token"))?;
            pos + 1
        };

        let mut jobs = Vec::new();
        for job_id in job_ids.iter().skip(start_index).take(page_size) {
            let Some(job) = self
                .state
                .job_store
                .get(job_id)
                .await
                .map_err(|err| Status::internal(format!("get job: {err:#}")))?
            else {
                continue;
            };
            let Some(start_request) = self
                .state
                .job_store
                .get_request(job_id)
                .await
                .map_err(|err| Status::internal(format!("get job request: {err:#}")))?
            else {
                continue;
            };
            jobs.push(job_to_pb(&job, &start_request));
        }

        let next_page_token = if jobs.len() == page_size {
            jobs.last()
                .map(|j| j.name.strip_prefix("jobs/").unwrap_or_default().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };

        Ok(TonicResponse::new(ListJobsResponse {
            jobs,
            next_page_token,
        }))
    }

    async fn generate_job_download_url(
        &self,
        request: Request<GenerateJobDownloadUrlRequest>,
    ) -> Result<TonicResponse<GenerateJobDownloadUrlResponse>, Status> {
        let job_id =
            job_id_from_name(&request.into_inner().name).map_err(Status::invalid_argument)?;
        let Some(job) = self
            .state
            .job_store
            .get(&job_id)
            .await
            .map_err(|err| Status::internal(format!("get job: {err:#}")))?
        else {
            return Err(Status::not_found("job not found"));
        };

        if job.status != JobStatus::Done || job.artifact_path.is_none() {
            return Err(Status::failed_precondition("artifact not ready"));
        }

        let url = self
            .state
            .artifact_store
            .generate_download_url(&job_id, self.state.signed_url_ttl_secs)
            .await
            .map_err(|err| Status::internal(format!("generate download url: {err:#}")))?;

        let expire_time = job
            .artifact_uri
            .as_deref()
            .is_some_and(|uri| uri.starts_with("gs://"))
            .then(|| {
                let expires_at = chrono::Utc::now()
                    + chrono::Duration::seconds(i64::from(self.state.signed_url_ttl_secs));
                timestamp_from_chrono(expires_at)
            });

        Ok(TonicResponse::new(GenerateJobDownloadUrlResponse {
            url,
            expire_time,
        }))
    }
}

#[derive(Clone)]
struct GrpcOperations {
    state: AppState,
}

#[tonic::async_trait]
impl LongrunningOperations for GrpcOperations {
    async fn list_operations(
        &self,
        _request: Request<ListOperationsRequest>,
    ) -> Result<TonicResponse<ListOperationsResponse>, Status> {
        Err(Status::unimplemented("ListOperations is not implemented"))
    }

    async fn get_operation(
        &self,
        request: Request<GetOperationRequest>,
    ) -> Result<TonicResponse<Operation>, Status> {
        let name = request.into_inner().name;
        let job_id = job_id_from_operation_name(&name).map_err(Status::invalid_argument)?;

        let Some(job) = self
            .state
            .job_store
            .get(&job_id)
            .await
            .map_err(|err| Status::internal(format!("get job: {err:#}")))?
        else {
            return Err(Status::not_found("operation not found"));
        };
        let Some(start_request) = self
            .state
            .job_store
            .get_request(&job_id)
            .await
            .map_err(|err| Status::internal(format!("get job request: {err:#}")))?
        else {
            return Err(Status::internal("job request not found"));
        };

        let metadata = CreateJobMetadata {
            job: job_name(&job_id),
            create_time: Some(timestamp_from_chrono(job.created_at)),
            start_time: job.started_at.map(timestamp_from_chrono),
            completion_time: job.finished_at.map(timestamp_from_chrono),
            progress_percent: job.progress_percent as i32,
            message: job.message.clone(),
        };

        let done = matches!(job.status, JobStatus::Done | JobStatus::Error);
        let result = match job.status {
            JobStatus::Done => {
                let pb_job = job_to_pb(&job, &start_request);
                Some(
                    sitebookify::google::longrunning::operation::Result::Response(pack_any(
                        "type.googleapis.com/sitebookify.v1.Job",
                        &pb_job,
                    )),
                )
            }
            JobStatus::Error => Some(sitebookify::google::longrunning::operation::Result::Error(
                RpcStatus {
                    code: 13, // INTERNAL
                    message: job.message.clone(),
                    details: Vec::new(),
                },
            )),
            JobStatus::Queued | JobStatus::Running => None,
        };

        Ok(TonicResponse::new(Operation {
            name,
            metadata: Some(pack_any(
                "type.googleapis.com/sitebookify.v1.CreateJobMetadata",
                &metadata,
            )),
            done,
            result,
        }))
    }

    async fn delete_operation(
        &self,
        _request: Request<DeleteOperationRequest>,
    ) -> Result<TonicResponse<()>, Status> {
        Err(Status::unimplemented("DeleteOperation is not implemented"))
    }

    async fn cancel_operation(
        &self,
        _request: Request<CancelOperationRequest>,
    ) -> Result<TonicResponse<()>, Status> {
        Err(Status::unimplemented("CancelOperation is not implemented"))
    }

    async fn wait_operation(
        &self,
        _request: Request<sitebookify::google::longrunning::WaitOperationRequest>,
    ) -> Result<TonicResponse<Operation>, Status> {
        Err(Status::unimplemented("WaitOperation is not implemented"))
    }
}

fn job_to_pb(job: &Job, start_request: &StartJobRequest) -> PbJob {
    let state = match job.status {
        JobStatus::Queued => PbJobState::Queued as i32,
        JobStatus::Running => PbJobState::Running as i32,
        JobStatus::Done => PbJobState::Done as i32,
        JobStatus::Error => PbJobState::Error as i32,
    };

    let artifact_uri = job
        .artifact_uri
        .clone()
        .or_else(|| {
            job.artifact_path
                .as_ref()
                .map(|p| format!("file://{}", p.display()))
        })
        .unwrap_or_default();

    PbJob {
        name: job_name(&job.job_id),
        spec: Some(job_spec_to_pb(start_request)),
        state,
        progress_percent: job.progress_percent as i32,
        message: job.message.clone(),
        create_time: Some(timestamp_from_chrono(job.created_at)),
        start_time: job.started_at.map(timestamp_from_chrono),
        completion_time: job.finished_at.map(timestamp_from_chrono),
        artifact_uri,
    }
}

fn job_spec_to_pb(start_request: &StartJobRequest) -> JobSpec {
    JobSpec {
        source_url: start_request.url.clone(),
        title: start_request.title.clone().unwrap_or_default(),
        max_pages: start_request.max_pages as i32,
        max_depth: start_request.max_depth as i32,
        concurrency: start_request.concurrency as i32,
        request_delay: Some(duration_from_ms(start_request.delay_ms)),
        language_code: start_request.language.clone(),
        tone: start_request.tone.clone(),
        toc_engine: engine_to_pb(start_request.toc_engine) as i32,
        render_engine: engine_to_pb(start_request.render_engine) as i32,
    }
}

fn engine_to_pb(engine: LlmEngine) -> Engine {
    match engine {
        LlmEngine::Noop => Engine::Noop,
        LlmEngine::Openai => Engine::Openai,
    }
}

fn engine_or_default(value: i32, default: LlmEngine) -> Result<LlmEngine, String> {
    match value {
        0 => Ok(default),
        x if x == Engine::Noop as i32 => Ok(LlmEngine::Noop),
        x if x == Engine::Openai as i32 => Ok(LlmEngine::Openai),
        other => Err(format!("unknown engine: {other}")),
    }
}

fn job_name(job_id: &str) -> String {
    format!("jobs/{job_id}")
}

fn operation_name(job_id: &str) -> String {
    format!("operations/{job_id}")
}

fn job_id_from_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let Some(job_id) = name.strip_prefix("jobs/") else {
        return Err("name must be of the form jobs/{job}".to_string());
    };
    uuid::Uuid::parse_str(job_id).map_err(|err| format!("invalid job name: {err}"))?;
    Ok(job_id.to_string())
}

fn job_id_from_operation_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let Some(job_id) = name.strip_prefix("operations/") else {
        return Err("name must be of the form operations/{operation}".to_string());
    };
    uuid::Uuid::parse_str(job_id).map_err(|err| format!("invalid operation name: {err}"))?;
    Ok(job_id.to_string())
}

fn timestamp_from_chrono(dt: chrono::DateTime<chrono::Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

fn duration_to_ms(d: &prost_types::Duration) -> Result<u64, String> {
    if d.seconds < 0 || d.nanos < 0 {
        return Err("request_delay must be >= 0".to_string());
    }
    let seconds = u64::try_from(d.seconds).map_err(|_| "request_delay is too large".to_string())?;
    let nanos =
        u64::try_from(d.nanos).map_err(|_| "request_delay nanos is too large".to_string())?;
    Ok(seconds
        .saturating_mul(1000)
        .saturating_add(nanos / 1_000_000))
}

fn duration_from_ms(ms: u64) -> prost_types::Duration {
    prost_types::Duration {
        seconds: (ms / 1000) as i64,
        nanos: ((ms % 1000) * 1_000_000) as i32,
    }
}

async fn list_local_job_ids(base_dir: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let jobs_dir = base_dir.join("jobs");
    let mut dir = match tokio::fs::read_dir(&jobs_dir).await {
        Ok(dir) => dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };

    let mut ids = Vec::new();
    while let Some(entry) = dir.next_entry().await? {
        let ty = entry.file_type().await?;
        if !ty.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if uuid::Uuid::parse_str(name.as_ref()).is_ok() {
            ids.push(name.to_string());
        }
    }
    Ok(ids)
}

fn pack_any(type_url: &str, msg: &impl prost::Message) -> prost_types::Any {
    prost_types::Any {
        type_url: type_url.to_string(),
        value: msg.encode_to_vec(),
    }
}

fn string_or_default(value: String, default: String) -> String {
    let v = value.trim();
    if v.is_empty() { default } else { v.to_string() }
}

fn i32_as_usize_or_default(value: i32, default: usize) -> Result<usize, String> {
    match value.cmp(&0) {
        std::cmp::Ordering::Less => Err("value must be >= 0".to_string()),
        std::cmp::Ordering::Equal => Ok(default),
        std::cmp::Ordering::Greater => Ok(value as usize),
    }
}

fn i32_as_u32_or_default(value: i32, default: u32) -> Result<u32, String> {
    match value.cmp(&0) {
        std::cmp::Ordering::Less => Err("value must be >= 0".to_string()),
        std::cmp::Ordering::Equal => Ok(default),
        std::cmp::Ordering::Greater => Ok(value as u32),
    }
}

trait IntoOptionString {
    fn into_option(self) -> Option<String>;
}

impl IntoOptionString for String {
    fn into_option(self) -> Option<String> {
        if self.trim().is_empty() {
            None
        } else {
            Some(self)
        }
    }
}
