import { anyUnpack } from "@bufbuild/protobuf/wkt";
import type { Client } from "@connectrpc/connect";
import { useEffect, useMemo, useState } from "react";
import {
  CreateJobMetadataSchema,
  Engine,
  SitebookifyService,
} from "../gen/sitebookify/v1/service_pb";
import {
  clearJobs,
  loadJobs,
  pruneJobsInStorage,
  removeJob,
  upsertJob,
  type StoredJob,
} from "../lib/jobHistory";

type Props = {
  client: Client<typeof SitebookifyService>;
  navigate: (to: string) => void;
};

type UiState = {
  busy: boolean;
  error: string | null;
};

function jobIdFromName(name: string): string {
  return name.startsWith("jobs/") ? name.slice("jobs/".length) : name;
}

function engineValue(e: string): Engine {
  const parsed = Number(e);
  if (!Number.isFinite(parsed)) return Engine.UNSPECIFIED;
  switch (parsed) {
    case Engine.NOOP:
    case Engine.OPENAI:
    case Engine.UNSPECIFIED:
      return parsed;
    default:
      return Engine.UNSPECIFIED;
  }
}

function formatTimestamp(ms: number): string {
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return String(ms);
  }
}

export function HomePage({ client, navigate }: Props) {
  const [query, setQuery] = useState("");
  const [languageCode, setLanguageCode] = useState("日本語");
  const [tone, setTone] = useState("丁寧");
  const [tocEngine, setTocEngine] = useState<Engine>(Engine.NOOP);
  const [renderEngine, setRenderEngine] = useState<Engine>(Engine.NOOP);
  const [{ busy, error }, setState] = useState<UiState>({ busy: false, error: null });
  const [jobHistory, setJobHistory] = useState<StoredJob[]>(() => loadJobs());

  const canStart = query.trim().length > 0 && !busy;

  const engineLabel =
    tocEngine === Engine.OPENAI || renderEngine === Engine.OPENAI ? "openai" : "noop";

  const recentJob = jobHistory.length > 0 ? jobHistory[0] : null;

  useEffect(() => {
    setJobHistory(pruneJobsInStorage());
  }, []);

  const banner = useMemo(() => {
    if (!recentJob) return null;
    const title =
      recentJob.state === "done"
        ? "前回のブックが完了しています"
        : recentJob.state === "error"
          ? "前回のブックが失敗しています"
          : "作成中のブックがあります";
    const subtitle =
      recentJob.query && recentJob.query.length > 0
        ? recentJob.query
        : `job_id: ${recentJob.jobId}`;
    return { title, subtitle, jobId: recentJob.jobId };
  }, [recentJob]);

  function refreshHistory() {
    setJobHistory(pruneJobsInStorage());
  }

  function clearHistory() {
    clearJobs();
    setJobHistory([]);
  }

  function removeFromHistory(jobId: string) {
    setJobHistory(removeJob(jobId));
  }

  async function start() {
    setState({ busy: true, error: null });
    try {
      const op = await client.createJob({
        job: {
          spec: {
            query: query.trim(),
            languageCode: languageCode.trim(),
            tone: tone.trim(),
            tocEngine,
            renderEngine,
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

      const jobId = jobIdFromName(resolvedJobName);
      const stored = {
        jobId,
        createdAtMs: Date.now(),
        query: query.trim(),
        state: "queued" as const,
        progressPercent: 0,
        message: "queued",
      };
      const nextHistory = upsertJob(stored);
      setJobHistory(nextHistory);

      navigate(`/jobs/${jobId}`);
    } catch (e) {
      setState({ busy: false, error: String(e) });
      return;
    }
    setState({ busy: false, error: null });
  }

  return (
    <div className="hero">
      {banner ? (
        <div className="banner">
          <div className="bannerTitle">{banner.title}</div>
          <div className="bannerSubtitle muted">{banner.subtitle}</div>
          <div className="row wrap bannerActions">
            <button className="small" onClick={() => navigate(`/jobs/${banner.jobId}`)}>
              進捗を見る
            </button>
          </div>
        </div>
      ) : null}

      <h1 className="title">
        Describe a topic,
        <br />
        get a textbook
      </h1>
      <p className="subtitle">
        テーマを入力するだけで、Webから情報を収集し本を自動生成します。
      </p>

      <div className="card">
        <div className="row formRow">
          <textarea
            placeholder="例: Rustの非同期プログラミングについて"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && canStart) {
                e.preventDefault();
                void start();
              }
            }}
            spellCheck={false}
            rows={3}
            style={{ resize: "vertical", flex: 1 }}
          />
          <button onClick={() => void start()} disabled={!canStart}>
            {busy ? "Starting…" : "Start"}
          </button>
        </div>

        <div className="settings">
          <div className="row wrap">
            <span className="pill">toc</span>
            <select
              className="control"
              value={tocEngine}
              onChange={(e) => setTocEngine(engineValue(e.target.value))}
            >
              <option value={Engine.NOOP}>noop</option>
              <option value={Engine.OPENAI}>openai</option>
            </select>

            <span className="pill">render</span>
            <select
              className="control"
              value={renderEngine}
              onChange={(e) => setRenderEngine(engineValue(e.target.value))}
            >
              <option value={Engine.NOOP}>noop</option>
              <option value={Engine.OPENAI}>openai</option>
            </select>

            <span className="pill">{engineLabel}</span>
          </div>

          <div className="row wrap">
            <span className="pill">language</span>
            <input
              className="control"
              type="text"
              value={languageCode}
              onChange={(e) => setLanguageCode(e.target.value)}
              placeholder="日本語 / English / …"
            />

            <span className="pill">tone</span>
            <input
              className="control"
              type="text"
              value={tone}
              onChange={(e) => setTone(e.target.value)}
              placeholder="丁寧 / casual / …"
            />
          </div>

          {engineLabel === "openai" ? (
            <div className="muted hint">
              OpenAI engine requires the API key on the server (see <code>OPENAI_API_KEY</code> /{" "}
              <code>SITEBOOKIFY_OPENAI_*</code> in README).
            </div>
          ) : null}
        </div>

        {error ? (
          <div className="row">
            <span className="pill error">error</span>
            <span className="error">{error}</span>
          </div>
        ) : null}
      </div>

      <div className="card">
        <div className="row wrap">
          <span className="pill">history</span>
          <span className="muted">ブラウザに保存 • 24h で自動削除</span>
          <button className="small" type="button" onClick={refreshHistory}>
            Refresh
          </button>
          <button
            className="small"
            type="button"
            onClick={clearHistory}
            disabled={jobHistory.length === 0}
          >
            Clear
          </button>
        </div>

        {jobHistory.length === 0 ? (
          <div className="muted hint">まだ実行履歴がありません。</div>
        ) : (
          <div className="status">
            {jobHistory.map((j) => (
              <div key={j.jobId} className="historyItem">
                <div className="row wrap">
                  <button
                    className="small"
                    type="button"
                    onClick={() => navigate(`/jobs/${j.jobId}`)}
                  >
                    Open
                  </button>

                  <span className="pill">job</span>
                  <code className="muted">{j.jobId}</code>

                  <span className="pill">status</span>
                  <span
                    className={
                      j.state === "error" ? "error" : j.state === "done" ? "success" : "muted"
                    }
                  >
                    {j.state ?? "unknown"}
                  </span>

                  {typeof j.progressPercent === "number" ? (
                    <span className="muted">{j.progressPercent}%</span>
                  ) : null}

                  <span className="muted">{formatTimestamp(j.createdAtMs)}</span>

                  {j.state === "done" ? (
                    <a
                      className="pillLink"
                      href={`/jobs/${j.jobId}/book.epub`}
                      download={`sitebookify-${j.jobId}.epub`}
                      title="EPUB: book.epub"
                    >
                      EPUB
                    </a>
                  ) : null}

                  <button className="small" type="button" onClick={() => removeFromHistory(j.jobId)}>
                    Remove
                  </button>
                </div>

                <div className="muted hint">
                  {[j.query?.trim(), j.message?.trim()]
                    .filter((v): v is string => Boolean(v && v.length > 0))
                    .join(" • ") || "—"}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="footer">
        Local MVP: `sitebookify-app` stores jobs under <code>workspace-app/jobs</code>. For Cloud Run,
        swap JobStore/ArtifactStore/Queue.
      </div>
    </div>
  );
}
