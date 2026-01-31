use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::error_handling::HandleErrorLayer;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{Html, Response};
use axum::routing::get;
use clap::Parser;
use http_body_util::BodyExt as _;
use tokio_util::io::ReaderStream;
use tonic::{Request, Response as TonicResponse, Status};
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use sitebookify::app::artifact_store::{ArtifactStore, LocalFsArtifactStore};
use sitebookify::app::job_store::{JobStore, LocalFsJobStore};
use sitebookify::app::model::{Job, JobStatus, StartJobRequest};
use sitebookify::app::queue::InProcessQueue;
use sitebookify::app::runner::{JobRunner, default_job_work_dir};
use sitebookify::cli::LlmEngine;
use sitebookify::grpc::v1::sitebookify_service_server::{
    SitebookifyService, SitebookifyServiceServer,
};
use sitebookify::grpc::v1::{
    GetDownloadUrlRequest, GetDownloadUrlResponse, GetJobRequest, Job as PbJob,
    JobStatus as PbJobStatus, StartCrawlRequest, StartCrawlResponse,
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
    let artifact_store: Arc<dyn ArtifactStore> =
        Arc::new(LocalFsArtifactStore::new(&args.data_dir));
    let runner = Arc::new(JobRunner::new(
        Arc::clone(&job_store),
        Arc::clone(&artifact_store),
    ));
    let state = AppState {
        base_dir: args.data_dir,
        job_store,
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

    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/artifacts/:job_id", get(download_artifact))
        .route_service("/sitebookify.v1.SitebookifyService/*rest", grpc_service)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    if args.web_dir.exists() {
        let index = args.web_dir.join("index.html");
        let static_files = ServeDir::new(args.web_dir).not_found_service(ServeFile::new(index));
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

#[derive(Clone)]
struct GrpcSitebookifyService {
    state: AppState,
}

#[tonic::async_trait]
impl SitebookifyService for GrpcSitebookifyService {
    async fn start_crawl(
        &self,
        request: Request<StartCrawlRequest>,
    ) -> Result<TonicResponse<StartCrawlResponse>, Status> {
        let req = request.into_inner();
        if req.url.trim().is_empty() {
            return Err(Status::invalid_argument("url is required"));
        }
        let url = url::Url::parse(req.url.trim())
            .map_err(|err| Status::invalid_argument(format!("invalid url: {err}")))?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(Status::invalid_argument("url must be http/https"));
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let work_dir = default_job_work_dir(&self.state.base_dir, &job_id);

        let start_request = StartJobRequest {
            url: url.to_string(),
            title: req.title.trim().to_string().into_option(),
            max_pages: usize_from_u32_or_default(
                req.max_pages,
                StartJobRequest::default_max_pages(),
            ),
            max_depth: u32_or_default(req.max_depth, StartJobRequest::default_max_depth()),
            concurrency: usize_from_u32_or_default(
                req.concurrency,
                StartJobRequest::default_concurrency(),
            ),
            delay_ms: u64_from_u32_or_default(req.delay_ms, StartJobRequest::default_delay_ms()),
            language: string_or_default(req.language, StartJobRequest::default_language()),
            tone: string_or_default(req.tone, StartJobRequest::default_tone()),
            toc_engine: parse_engine(req.toc_engine).unwrap_or(StartJobRequest::default_engine()),
            render_engine: parse_engine(req.render_engine)
                .unwrap_or(StartJobRequest::default_engine()),
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

        Ok(TonicResponse::new(StartCrawlResponse { job_id }))
    }

    async fn get_job(
        &self,
        request: Request<GetJobRequest>,
    ) -> Result<TonicResponse<PbJob>, Status> {
        let job_id = request.into_inner().job_id;
        let Some(job) = self
            .state
            .job_store
            .get(&job_id)
            .await
            .map_err(|err| Status::internal(format!("get job: {err:#}")))?
        else {
            return Err(Status::not_found("job not found"));
        };

        Ok(TonicResponse::new(job_to_pb(&job, &job_id)))
    }

    async fn get_download_url(
        &self,
        request: Request<GetDownloadUrlRequest>,
    ) -> Result<TonicResponse<GetDownloadUrlResponse>, Status> {
        let job_id = request.into_inner().job_id;
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

        Ok(TonicResponse::new(GetDownloadUrlResponse {
            url: format!("/artifacts/{job_id}"),
            expires_sec: 0,
        }))
    }
}

fn job_to_pb(job: &Job, job_id: &str) -> PbJob {
    let status = match job.status {
        JobStatus::Queued => PbJobStatus::Queued as i32,
        JobStatus::Running => PbJobStatus::Running as i32,
        JobStatus::Done => PbJobStatus::Done as i32,
        JobStatus::Error => PbJobStatus::Error as i32,
    };

    let artifact_url = if job.status == JobStatus::Done && job.artifact_path.is_some() {
        format!("/artifacts/{job_id}")
    } else {
        String::new()
    };

    PbJob {
        job_id: job.job_id.clone(),
        status,
        progress_percent: job.progress_percent,
        message: job.message.clone(),
        artifact_url,
    }
}

fn parse_engine(value: String) -> Option<LlmEngine> {
    match value.trim().to_lowercase().as_str() {
        "" => None,
        "noop" => Some(LlmEngine::Noop),
        "codex" => Some(LlmEngine::Codex),
        other => {
            tracing::warn!(engine = other, "unknown engine; falling back to default");
            None
        }
    }
}

fn string_or_default(value: String, default: String) -> String {
    let v = value.trim();
    if v.is_empty() { default } else { v.to_string() }
}

fn u32_or_default(value: u32, default: u32) -> u32 {
    if value == 0 { default } else { value }
}

fn u64_from_u32_or_default(value: u32, default: u64) -> u64 {
    if value == 0 { default } else { value as u64 }
}

fn usize_from_u32_or_default(value: u32, default: usize) -> usize {
    if value == 0 { default } else { value as usize }
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
