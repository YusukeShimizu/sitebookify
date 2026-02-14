#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  tf-with-ci-sha.sh [options] <plan|apply> [terraform args...]

Options:
  --workflow <file>         GitHub workflow file used as SHA source (default: deploy-cloudrun.yml)
  --branch <name>           Branch for workflow run lookup (default: main)
  --allow-rollback          Allow applying older SHA than current Cloud Run image
  --skip-cloudrun-check     Skip pre-apply Cloud Run image diff check
  -h, --help                Show this help

Environment overrides:
  GITHUB_REPOSITORY         owner/repo (auto-detected from git remote when omitted)
  GCP_PROJECT_ID            GCP project id (fallback: TF_VAR_project_id or terraform.tfvars)
  GCP_REGION                GCP region (fallback: TF_VAR_region or terraform.tfvars)
  CLOUD_RUN_SERVICE         API service name (fallback: TF_VAR_service_name or terraform.tfvars or sitebookify)
  CLOUD_RUN_WORKER_SERVICE  Worker service name (fallback: TF_VAR_worker_service_name or terraform.tfvars or sitebookify-worker)
  TFVARS_FILE               terraform vars file path (default: infra/terraform/cloudrun-public-gcs/terraform.tfvars)
USAGE
}

require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "[ERROR] Required command not found: $cmd" >&2
    exit 1
  fi
}

repo_slug_from_remote() {
  local remote_url
  remote_url="$(git -C "$REPO_ROOT" remote get-url origin 2>/dev/null || true)"
  if [[ -z "$remote_url" ]]; then
    return 1
  fi

  case "$remote_url" in
    git@github.com:*)
      printf '%s\n' "${remote_url#git@github.com:}" | sed -E 's/\.git$//'
      return 0
      ;;
    https://github.com/*)
      printf '%s\n' "${remote_url#https://github.com/}" | sed -E 's/\.git$//'
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

tfvars_get_string() {
  local key="$1"
  local file="$TFVARS_FILE"
  if [[ ! -f "$file" ]]; then
    return 0
  fi

  python3 - "$file" "$key" <<'PY'
import os
import re
import sys

path, key = sys.argv[1], sys.argv[2]
if not os.path.exists(path):
    raise SystemExit(0)

pattern = re.compile(r"^\s*" + re.escape(key) + r"\s*=\s*\"([^\"]+)\"\s*(?:#.*)?$")
with open(path, encoding="utf-8") as f:
    for line in f:
        match = pattern.match(line)
        if match:
            print(match.group(1).strip())
            break
PY
}

extract_sha_from_image() {
  local image="$1"
  if [[ "$image" =~ :sha-([0-9a-fA-F]{40})$ ]]; then
    printf '%s\n' "${BASH_REMATCH[1],,}"
  fi
}

classify_sha_delta() {
  local current_sha="$1"
  local target_sha="$2"

  if [[ "$current_sha" == "$target_sha" ]]; then
    echo "same"
    return 0
  fi

  if ! git -C "$REPO_ROOT" cat-file -e "${current_sha}^{commit}" 2>/dev/null || \
     ! git -C "$REPO_ROOT" cat-file -e "${target_sha}^{commit}" 2>/dev/null; then
    echo "unknown"
    return 0
  fi

  if git -C "$REPO_ROOT" merge-base --is-ancestor "$target_sha" "$current_sha"; then
    echo "rollback"
    return 0
  fi

  if git -C "$REPO_ROOT" merge-base --is-ancestor "$current_sha" "$target_sha"; then
    echo "forward"
    return 0
  fi

  echo "diverged"
}

check_cloud_run_service() {
  local service="$1"
  local current_image
  current_image="$(gcloud run services describe "$service" \
    --project "$GCP_PROJECT_ID" \
    --region "$GCP_REGION" \
    --format='value(spec.template.spec.containers[0].image)' 2>/dev/null || true)"

  if [[ -z "$current_image" ]]; then
    echo "[WARN] Cloud Run service '$service' not found or image field unavailable; skipping diff check."
    return 0
  fi

  local current_sha
  current_sha="$(extract_sha_from_image "$current_image")"
  if [[ -z "$current_sha" ]]; then
    echo "[WARN] Could not extract ':sha-<commit>' tag from '$service' image: $current_image"
    return 0
  fi

  if [[ "$current_sha" == "$DEPLOY_SHA" ]]; then
    echo "[OK] $service image already matches deploy_sha: $DEPLOY_SHA"
    return 0
  fi

  local delta
  delta="$(classify_sha_delta "$current_sha" "$DEPLOY_SHA")"
  echo "[INFO] $service image differs: current=$current_sha target=$DEPLOY_SHA ($delta)"

  if [[ "$delta" == "rollback" && "$ALLOW_ROLLBACK" != "true" ]]; then
    ROLLBACK_DETECTED=true
  fi
}

WORKFLOW_FILE="deploy-cloudrun.yml"
WORKFLOW_BRANCH="main"
ALLOW_ROLLBACK="false"
SKIP_CLOUDRUN_CHECK="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --workflow)
      WORKFLOW_FILE="${2:-}"
      shift 2
      ;;
    --branch)
      WORKFLOW_BRANCH="${2:-}"
      shift 2
      ;;
    --allow-rollback)
      ALLOW_ROLLBACK="true"
      shift
      ;;
    --skip-cloudrun-check)
      SKIP_CLOUDRUN_CHECK="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
  esac
done

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

TF_COMMAND="$1"
shift
if [[ "$TF_COMMAND" != "plan" && "$TF_COMMAND" != "apply" ]]; then
  echo "[ERROR] First argument must be 'plan' or 'apply'." >&2
  usage
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TF_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$TF_DIR/../../.." && pwd)"
TFVARS_FILE="${TFVARS_FILE:-$TF_DIR/terraform.tfvars}"

require_cmd gh
require_cmd git
require_cmd python3
require_cmd terraform

REPO_SLUG="${GITHUB_REPOSITORY:-}"
if [[ -z "$REPO_SLUG" ]]; then
  REPO_SLUG="$(repo_slug_from_remote || true)"
fi
if [[ -z "$REPO_SLUG" ]]; then
  echo "[ERROR] Could not determine GitHub repository slug. Set GITHUB_REPOSITORY=owner/repo." >&2
  exit 1
fi

runs_json="$(gh api -H "Accept: application/vnd.github+json" \
  "/repos/${REPO_SLUG}/actions/workflows/${WORKFLOW_FILE}/runs" \
  -f branch="$WORKFLOW_BRANCH" \
  -f event="push" \
  -f status="success" \
  -f per_page=1)"

DEPLOY_SHA="$(printf '%s' "$runs_json" | python3 -c '
import json
import re
import sys

data = json.load(sys.stdin)
runs = data.get("workflow_runs") or []
if not runs:
    raise SystemExit(0)
sha = (runs[0].get("head_sha") or "").strip().lower()
if not re.fullmatch(r"[0-9a-f]{40}", sha):
    raise SystemExit(0)
print(sha)
')"

if [[ -z "$DEPLOY_SHA" ]]; then
  echo "[ERROR] Could not resolve latest successful workflow SHA from '${WORKFLOW_FILE}' on branch '${WORKFLOW_BRANCH}'." >&2
  exit 1
fi

echo "[INFO] Using deploy_sha from CI: $DEPLOY_SHA"

if [[ "$SKIP_CLOUDRUN_CHECK" != "true" ]]; then
  require_cmd gcloud

  GCP_PROJECT_ID="${GCP_PROJECT_ID:-${TF_VAR_project_id:-$(tfvars_get_string project_id)}}"
  GCP_REGION="${GCP_REGION:-${TF_VAR_region:-$(tfvars_get_string region)}}"
  CLOUD_RUN_SERVICE="${CLOUD_RUN_SERVICE:-${TF_VAR_service_name:-$(tfvars_get_string service_name)}}"
  CLOUD_RUN_WORKER_SERVICE="${CLOUD_RUN_WORKER_SERVICE:-${TF_VAR_worker_service_name:-$(tfvars_get_string worker_service_name)}}"

  CLOUD_RUN_SERVICE="${CLOUD_RUN_SERVICE:-sitebookify}"
  CLOUD_RUN_WORKER_SERVICE="${CLOUD_RUN_WORKER_SERVICE:-sitebookify-worker}"

  if [[ -z "$GCP_PROJECT_ID" || -z "$GCP_REGION" ]]; then
    echo "[ERROR] Cloud Run image diff check requires project/region. Set GCP_PROJECT_ID and GCP_REGION, or use --skip-cloudrun-check." >&2
    exit 1
  fi

  git -C "$REPO_ROOT" fetch --quiet origin "$WORKFLOW_BRANCH" || true

  ROLLBACK_DETECTED=false
  check_cloud_run_service "$CLOUD_RUN_SERVICE"
  check_cloud_run_service "$CLOUD_RUN_WORKER_SERVICE"

  if [[ "$ROLLBACK_DETECTED" == "true" ]]; then
    echo "[ERROR] Rollback risk detected: target deploy_sha appears older than current Cloud Run image SHA." >&2
    echo "        If intentional, rerun with --allow-rollback." >&2
    exit 1
  fi
fi

terraform -chdir="$TF_DIR" "$TF_COMMAND" -var="deploy_sha=$DEPLOY_SHA" "$@"
