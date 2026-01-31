import { useEffect, useMemo, useState } from "react";
import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { anyUnpack } from "@bufbuild/protobuf/wkt";
import {
  CreateJobMetadataSchema,
  Engine,
  Job_State,
  SitebookifyService,
  type Job,
} from "./gen/sitebookify/v1/service_pb";

type UiState = {
  jobName: string | null;
  job: Job | null;
  downloadUrl: string | null;
  error: string | null;
  busy: boolean;
};

export default function App() {
  const client = useMemo(() => {
    const transport = createGrpcWebTransport({
      baseUrl: "",
    });
    return createClient(SitebookifyService, transport);
  }, []);

  const [url, setUrl] = useState("https://example.com/docs/");
  const [{ jobName, job, downloadUrl, error, busy }, setState] = useState<UiState>({
    jobName: null,
    job: null,
    downloadUrl: null,
    error: null,
    busy: false,
  });

  const canStart = url.trim().length > 0 && !busy;

  async function start() {
    setState((s) => ({ ...s, busy: true, error: null, downloadUrl: null, job: null, jobName: null }));
    try {
      const op = await client.createJob({
        job: {
          spec: {
            sourceUrl: url.trim(),
            tocEngine: Engine.NOOP,
            renderEngine: Engine.NOOP,
          },
        },
        jobId: "",
      });

      const jobNameFromMetadata = op.metadata
        ? anyUnpack(op.metadata, CreateJobMetadataSchema)?.job ?? null
        : null;
      const jobNameFromOp = op.name.startsWith("operations/")
        ? `jobs/${op.name.slice("operations/".length)}`
        : null;

      const resolvedJobName = jobNameFromMetadata ?? jobNameFromOp;
      if (!resolvedJobName) {
        throw new Error(`CreateJob returned an operation without a job reference: ${op.name}`);
      }

      setState((s) => ({ ...s, jobName: resolvedJobName, busy: false }));
    } catch (e) {
      setState((s) => ({ ...s, busy: false, error: String(e) }));
    }
  }

  useEffect(() => {
    if (!jobName) return;
    if (downloadUrl) return;
    let stopped = false;

    const tick = async () => {
      try {
        const j = await client.getJob({ name: jobName });
        if (stopped) return;
        setState((s) => ({ ...s, job: j }));

        if (j.state === Job_State.DONE) {
          const dl = await client.generateJobDownloadUrl({ name: jobName });
          if (stopped) return;
          setState((s) => ({ ...s, downloadUrl: dl.url }));
        }
      } catch (e) {
        if (stopped) return;
        setState((s) => ({ ...s, error: String(e) }));
      }
    };

    void tick();
    const id = window.setInterval(() => void tick(), 1500);
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [client, jobName, downloadUrl]);

  const statusText = (() => {
    if (!job) return "—";
    switch (job.state) {
      case Job_State.QUEUED:
        return "queued";
      case Job_State.RUNNING:
        return "running";
      case Job_State.DONE:
        return "done";
      case Job_State.ERROR:
        return "error";
      default:
        return "unknown";
    }
  })();

  return (
    <div className="container">
      <div className="topbar">
        <div className="brand">{">_ sitebookify"}</div>
        <div className="pill">gRPC-Web • local FS • noop</div>
      </div>

      <div className="hero">
        <h1 className="title">
          Turn docs into a
          <br />
          textbook Markdown
        </h1>
        <p className="subtitle">
          Paste a start URL. Sitebookify crawls, extracts, builds an mdBook, then bundles it into a
          single <code>book.md</code> (+assets).
        </p>

        <div className="card">
          <div className="row">
            <input
              type="url"
              placeholder="https://example.com/docs/"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && canStart) void start();
              }}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
            />
            <button onClick={() => void start()} disabled={!canStart}>
              {busy ? "Starting…" : "Start"}
            </button>
          </div>

          <div className="status">
            <div className="row">
              <span className="pill">job</span>
              <span className="muted">{jobName ?? "—"}</span>
            </div>
            <div className="row">
              <span className="pill">status</span>
              <span className={job?.state === Job_State.ERROR ? "error" : "muted"}>
                {statusText}
              </span>
              {job ? <span className="muted">• {job.message}</span> : null}
            </div>

            <div className="progress" aria-label="progress">
              <div style={{ width: `${job?.progressPercent ?? 0}%` }} />
            </div>
            <div className="muted">{job ? `${job.progressPercent}%` : ""}</div>

            {downloadUrl ? (
              <div className="row">
                <span className="pill success">artifact</span>
                <a href={downloadUrl}>Download zip</a>
              </div>
            ) : null}

            {error ? (
              <div className="row">
                <span className="pill error">error</span>
                <span className="error">{error}</span>
              </div>
            ) : null}
          </div>
        </div>

        <div className="footer">
          Local MVP: `sitebookify-app` stores jobs under <code>workspace-app/jobs</code>. For Cloud Run,
          swap JobStore/ArtifactStore/Queue.
        </div>
      </div>
    </div>
  );
}
