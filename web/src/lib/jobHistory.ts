const STORAGE_KEY = "sitebookify.job_history.v1";
const MAX_AGE_MS = 24 * 60 * 60 * 1000;
const MAX_ITEMS = 20;

export type StoredJobState = "queued" | "running" | "done" | "error" | "unknown";

export type StoredJob = {
  jobId: string;
  createdAtMs: number;
  sourceUrl?: string;
  state?: StoredJobState;
  progressPercent?: number;
  message?: string;
};

function safeJsonParse(value: string): unknown {
  try {
    return JSON.parse(value);
  } catch {
    return null;
  }
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null;
}

function normalizeJob(v: unknown): StoredJob | null {
  if (!isRecord(v)) return null;

  const jobId = typeof v.jobId === "string" ? v.jobId.trim() : "";
  if (jobId.length === 0) return null;

  const createdAtMsRaw = v.createdAtMs;
  const createdAtMs =
    typeof createdAtMsRaw === "number" && Number.isFinite(createdAtMsRaw)
      ? createdAtMsRaw
      : NaN;
  if (!Number.isFinite(createdAtMs) || createdAtMs <= 0) return null;

  const sourceUrl = typeof v.sourceUrl === "string" ? v.sourceUrl.trim() : undefined;
  const message = typeof v.message === "string" ? v.message : undefined;
  const progressPercentRaw = v.progressPercent;
  const progressPercent =
    typeof progressPercentRaw === "number" && Number.isFinite(progressPercentRaw)
      ? Math.max(0, Math.min(100, progressPercentRaw))
      : undefined;

  const state =
    v.state === "queued" || v.state === "running" || v.state === "done" || v.state === "error"
      ? v.state
      : "unknown";

  return { jobId, createdAtMs, sourceUrl, state, progressPercent, message };
}

function pruneExpired(jobs: StoredJob[], nowMs: number): StoredJob[] {
  return jobs.filter((j) => nowMs - j.createdAtMs <= MAX_AGE_MS);
}

export function loadJobs(nowMs = Date.now()): StoredJob[] {
  const raw = window.localStorage.getItem(STORAGE_KEY);
  if (!raw) return [];

  const parsed = safeJsonParse(raw);
  if (!Array.isArray(parsed)) return [];

  const normalized: StoredJob[] = [];
  for (const item of parsed) {
    const job = normalizeJob(item);
    if (job) normalized.push(job);
  }

  const pruned = pruneExpired(normalized, nowMs);
  pruned.sort((a, b) => b.createdAtMs - a.createdAtMs);
  return pruned.slice(0, MAX_ITEMS);
}

export function saveJobs(jobs: StoredJob[]) {
  const list = [...jobs];
  list.sort((a, b) => b.createdAtMs - a.createdAtMs);
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(list.slice(0, MAX_ITEMS)));
}

export function upsertJob(update: StoredJob, nowMs = Date.now()): StoredJob[] {
  const existing = loadJobs(nowMs);
  const next = existing.filter((j) => j.jobId !== update.jobId);
  next.unshift(update);
  saveJobs(next);
  return next;
}

export function getMostRecentJob(nowMs = Date.now()): StoredJob | null {
  const jobs = loadJobs(nowMs);
  return jobs.length > 0 ? jobs[0] : null;
}

export function getJobById(jobId: string, nowMs = Date.now()): StoredJob | null {
  const id = jobId.trim();
  if (id.length === 0) return null;
  const jobs = loadJobs(nowMs);
  return jobs.find((j) => j.jobId === id) ?? null;
}

