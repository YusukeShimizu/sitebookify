# Design: Cloud Run Observability Skill

Date: 2026-02-12
Target card: `https://trello.com/c/onVToQaO`
Related card: `https://trello.com/c/Xvlu1U1f`

## Goal

Define a reusable skill design that analyzes Cloud Run incidents and returns:

- evidence-based incident summary
- root-cause hypotheses with confidence
- immediate and permanent remediation proposals
- monitoring and alerting improvements

## Scope

- Design only (no runtime scripts or infrastructure changes)
- Two-layer design:
  - generic Cloud Run profile
  - sitebookify-specific profile for known execution-model risks

## Deliverables

Skill files were created at:

- `.codex/skills/cloud-run-observability/SKILL.md`
- `.codex/skills/cloud-run-observability/agents/openai.yaml`
- `.codex/skills/cloud-run-observability/references/contracts.md`
- `.codex/skills/cloud-run-observability/references/playbook.md`
- `.codex/skills/cloud-run-observability/references/report-template.md`
- `.codex/skills/cloud-run-observability/references/sitebookify-profile.md`

## Public Contracts

Input contract defines:

- `project_id`, `region`, `target_type`, `target_name`, `time_range`
- optional `request_id`, `job_id`, `revision`, `profile`

Output contract defines:

- `facts` (`EvidenceRecord[]`)
- `hypotheses` (with confidence and evidence linkage)
- `recommendations` (short and long term)
- `monitoring_changes` (metric/condition/threshold/rationale)

## sitebookify Profile Anchors

- `src/bin/sitebookify-app.rs`
- `src/app/queue.rs`
- `infra/terraform/cloudrun-public-gcs/main.tf`
- `Dockerfile`

The profile mandates checks for request-lifecycle coupling, revision termination
signals, runtime config alignment, and job state progression.

## Validation Notes

- `skill-creator` helper scripts (`init_skill.py`, `quick_validate.py`) could not
  run in this environment due missing Python dependency `yaml` (`PyYAML`).
- Frontmatter structure was checked with a fallback local validation command.

## Acceptance Mapping

- Execution flow documentation: covered by `references/playbook.md`.
- Incident output template: covered by `references/report-template.md`.
- Security and evidence discipline: enforced in `SKILL.md` and contracts.
- sitebookify linkage to `Xvlu1U1f`: covered by `references/sitebookify-profile.md`.
