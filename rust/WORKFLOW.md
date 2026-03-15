---
tracker:
  kind: github
  owner: "ridermw"
  repo: "rusty"
  project_number: 5
  active_issue_labels:
    - "Todo"
    - "InProgress"
    - "HumanReview"
    - "Merging"
    - "Rework"
  terminal_issue_labels:
    - "Done"
    - "closed"
    - "Cancelled"
    - "Duplicate"
  project:
    enabled: true
    owner_type: "user"
    owner_name: "ridermw"
    project_number: 5
    status_field: "Status"
    status_field_id: "PVTSSF_lAHOAFl5zs4BRqBNzg_a8jM"
    project_id: "PVT_kwHOAFl5zs4BRqBN"
    active_states:
      - "Todo"
      - "InProgress"
      - "HumanReview"
      - "Merging"
      - "Rework"
    terminal_states:
      - "Done"
      - "Cancelled"
      - "Duplicate"
polling:
  interval_ms: 30000
workspace:
  root: $RUSTY_WORKSPACE_ROOT
hooks:
  after_create: |
    git clone --depth 1 https://github.com/ridermw/rusty .
  before_remove: |
    echo "workspace cleanup"
agent:
  max_concurrent_agents: 3
  max_turns: 20
copilot:
  command: copilot
  chat_command: copilot chat
  approval_policy: never
  thread_sandbox: workspace_write
  turn_sandbox_policy:
    type: workspaceWrite
github:
  cli_command: gh
  default_branch: main
  required_pr_label: "rusty"
---

You are working on GitHub issue `{{ issue.number }}`.

{% if attempt %}
Continuation context:

1. This is retry attempt #{{ attempt }} because the issue is still in an active state.
2. Resume from the current workspace state instead of restarting from scratch.
3. Do not repeat already completed investigation or validation unless needed for new code changes.
4. Do not end the turn while the issue remains in an active state unless blocked by missing required permissions, auth, or secrets.
{% endif %}

Issue context:
Number: {{ issue.number }}
Title: {{ issue.title }}
Current status: {{ issue.state }}
Labels: {{ issue.labels }}
URL: {{ issue.url }}

Description:
{% if issue.body %}
{{ issue.body }}
{% else %}
No description provided.
{% endif %}

Instructions:

1. This is an unattended orchestration session. Never ask a human to perform follow up actions.
2. Only stop early for a true blocker, such as missing required auth, permissions, or secrets. If blocked, record it in the workpad and move the issue according to workflow.
3. Final message must report completed actions and blockers only. Do not include next steps for the user.

Work only in the provided repository copy. Do not touch any other path.

## Prerequisite: GitHub CLI and Copilot CLI access

The agent should be able to use GitHub through `gh` and Copilot CLI through `copilot` or the locally configured Copilot CLI entrypoint. If GitHub access is unavailable, stop only after exhausting documented fallback options. If Copilot CLI is unavailable, continue the workflow without it and record that in the workpad.

## Default posture

1. Start by determining the issue's current status, then follow the matching flow for that status.
2. Start every task by opening the tracking workpad comment and bringing it up to date before doing new implementation work.
3. Spend extra effort up front on planning and verification design before implementation.
4. Reproduce first. Always confirm the current behavior or issue signal before changing code so the fix target is explicit.
5. Keep issue metadata current through labels, linked PRs, checklists, and acceptance criteria.
6. Treat a single persistent GitHub issue comment as the source of truth for progress.
7. Use that single workpad comment for all progress and handoff notes. Do not post separate done or summary comments.
8. Treat any issue authored `Validation`, `Test Plan`, or `Testing` section as required acceptance input. Mirror it in the workpad and execute it before considering the work complete.
9. When meaningful out of scope improvements are discovered during execution, file a separate GitHub issue instead of expanding scope. The follow up issue must include a clear title, description, acceptance criteria, and a link back to the current issue.
10. Move status only when the matching quality bar is met.
11. Operate autonomously end to end unless blocked by missing requirements, secrets, or permissions.
12. Use the blocked access escape hatch only for true external blockers after exhausting documented fallbacks.

## Related skills

1. `github`: interact with GitHub issues, pull requests, comments, labels, and checks through `gh`.
2. `commit`: produce clean, logical commits during implementation.
3. `push`: keep remote branch current and publish updates.
4. `pull`: keep branch updated with latest `origin/main` before handoff.
5. `land`: when the issue reaches `Merging`, explicitly open and follow `.github/skills/land/SKILL.md`, which includes the land loop.
6. `copilot`: use Copilot CLI for repository understanding, code changes, test scaffolding, and review assistance when it improves speed or quality.

## Status map

Use the GitHub Project status field as the primary state mechanism. Update status using the project CLI commands documented below. Do NOT use labels for state tracking.

1. `Backlog`
   Out of scope for this workflow. Do not modify.

2. `Todo`
   Queued. Immediately transition to `InProgress` before active work.
   If a PR is already attached, treat as feedback or rework loop. Run the full PR feedback sweep, address or explicitly push back, revalidate, and return to `HumanReview`.

3. `InProgress`
   Implementation actively underway.

4. `HumanReview`
   PR is attached and validated. Waiting on human approval.

5. `Merging`
   Approved by human. Execute the `land` skill flow. Do not call `gh pr merge` directly unless the land skill explicitly requires it.

6. `Rework`
   Reviewer requested changes. Planning plus implementation required.

7. `Done`
   Terminal state. No further action required.

## Step 0: Determine current issue state and route

1. Fetch the issue by explicit issue number.
2. Read the current state from the GitHub Project status field.
3. Route to the matching flow:
   1. `Backlog`
      Do not modify issue content or state. Stop and wait for human to move it to `Todo`.
   2. `Todo`
      Immediately move to `InProgress`, then ensure bootstrap workpad comment exists, then start execution flow.
      If a PR is already attached, start by reviewing all open PR comments and deciding required changes versus explicit pushback responses.
   3. `InProgress`
      Continue execution flow from current workpad comment.
   4. `HumanReview`
      Wait and poll for decision or review updates.
   5. `Merging`
      On entry, open and follow `.github/skills/land/SKILL.md`. Do not call `gh pr merge` directly unless that skill requires it.
   6. `Rework`
      Run rework flow.
   7. `Done`
      Do nothing and shut down.
4. Check whether a PR already exists for the current branch and whether it is closed.
   1. If a branch PR exists and is `CLOSED` or `MERGED`, treat prior branch work as non reusable for this run.
   2. Create a fresh branch from `origin/main` and restart execution flow as a new attempt.
5. For `Todo` issues, do startup sequencing in this exact order:
   1. Update project status to `InProgress` using the project CLI commands below
   2. Find or create `## Rusty Workpad` bootstrap comment
   3. Only then begin analysis, planning, and implementation work
6. Add a short comment if state and issue content are inconsistent, then proceed with the safest flow.

## Step 1: Start or continue execution

1. Find or create a single persistent workpad comment for the issue:
   1. Search existing comments for a marker header: `## Rusty Workpad`
   2. Reuse that comment if found
   3. If not found, create one workpad comment and use it for all updates
   4. Persist the workpad comment ID and only write progress updates to that ID
2. If arriving from `Todo`, do not delay on additional status transitions. The issue should already be `InProgress` before this step begins.
3. Immediately reconcile the workpad before new edits:
   1. Check off items that are already done
   2. Expand or fix the plan so it is comprehensive for current scope
   3. Ensure `Acceptance Criteria` and `Validation` are current and still make sense for the task
4. Start work by writing or updating a hierarchical plan in the workpad comment.
5. Ensure the workpad includes a compact environment stamp at the top as a code fence line:
   1. Format: `<host>:<abs_workdir>@<short_sha>`
   2. Example: `devbox01:/home/dev/code/rusty_workspaces/ISSUE32@7bdde33bc`
   3. Do not include metadata already inferable from issue fields, such as issue number, status, branch, or PR link
6. Add explicit acceptance criteria and TODOs in checklist form in the same comment.
   1. If changes are user facing, include a UI walkthrough acceptance criterion that describes the end to end user path to validate.
   2. If changes touch app files or app behavior, add explicit app specific flow checks to `Acceptance Criteria`.
   3. If the issue body or comments include `Validation`, `Test Plan`, or `Testing` sections, copy those requirements into the workpad `Acceptance Criteria` and `Validation` sections as required checkboxes.
7. Run a principal style self review of the plan and refine it in the comment.
8. Before implementing, capture a concrete reproduction signal and record it in the workpad `Notes` section with command output, screenshot, or deterministic runtime behavior.
9. Run the `pull` skill to sync with latest `origin/main` before any code edits, then record the pull and sync result in the workpad `Notes`.
   1. Include a `pull skill evidence` note with merge source, result, and resulting `HEAD` short SHA
10. Compact context and proceed to execution.

## PR feedback sweep protocol

When an issue has an attached PR, run this protocol before moving to `HumanReview`:

1. Identify the PR number from linked issues, branch metadata, or issue references.
2. Gather feedback from all channels:
   1. Top level PR comments using `gh pr view --comments`
   2. Inline review comments using `gh api repos/<owner>/<repo>/pulls/<pr>/comments`
   3. Review summaries and states using `gh pr view --json reviews`
3. Treat every actionable reviewer comment, human or bot, including inline review comments, as blocking until one of these is true:
   1. code, tests, or docs are updated to address it
   2. explicit, justified pushback reply is posted on that thread
4. Update the workpad plan and checklist to include each feedback item and its resolution status.
5. Re run validation after feedback driven changes and push updates.
6. Repeat this sweep until there are no outstanding actionable comments.

## Blocked access escape hatch

Use this only when completion is blocked by missing required tools or missing auth or permissions that cannot be resolved in session.

1. GitHub is not a valid blocker by default. Always try fallback strategies first, then continue publish or review flow.
2. Do not move to `HumanReview` for GitHub access or auth until all fallback strategies have been attempted and documented in the workpad.
3. If a non GitHub required tool is missing, or required non GitHub auth is unavailable, move the issue to `HumanReview` with a short blocker brief in the workpad that includes:
   1. what is missing
   2. why it blocks required acceptance or validation
   3. exact human action needed to unblock
4. Keep the brief concise and action oriented. Do not add extra top level comments outside the workpad.

## Step 2: Execution phase

1. Determine current repo state, including branch, `git status`, and `HEAD`, and verify the kickoff pull sync result is already recorded in the workpad before implementation continues.
2. If current issue state is `Todo`, move it to `InProgress`. Otherwise leave the current state unchanged.
3. Load the existing workpad comment and treat it as the active execution checklist.
   1. Edit it liberally whenever reality changes, including scope, risks, validation approach, or discovered tasks.
4. Implement against the hierarchical TODOs and keep the comment current:
   1. Check off completed items
   2. Add newly discovered items in the appropriate section
   3. Keep parent and child structure intact as scope evolves
   4. Update the workpad immediately after each meaningful milestone
   5. Never leave completed work unchecked in the plan
   6. For issues that started as `Todo` with an attached PR, run the full PR feedback sweep protocol immediately after kickoff and before new feature work
5. Run validation and tests required for the scope.
   1. Mandatory gate: execute all issue provided `Validation`, `Test Plan`, or `Testing` requirements when present
   2. Prefer a targeted proof that directly demonstrates the behavior you changed
   3. You may make temporary local proof edits to validate assumptions when this increases confidence
   4. Revert every temporary proof edit before commit or push
   5. Document these temporary proof steps and outcomes in the workpad `Validation` and `Notes` sections
   6. If app touching, run runtime validation and capture evidence for the PR
6. Re check all acceptance criteria and close any gaps.
7. Before every `git push` attempt, run the required validation for your scope and confirm it passes. If it fails, address issues and rerun until green, then commit and push changes.
8. Attach PR URL to the issue using GitHub issue linking conventions and cross references.
   1. Ensure the PR has label `rusty`
9. Merge latest `origin/main` into branch, resolve conflicts, and rerun checks.
10. Update the workpad comment with final checklist status and validation notes.
    1. Mark completed plan, acceptance, and validation items as checked
    2. Add final handoff notes with commit and validation summary in the same workpad comment
    3. Do not include PR URL in the workpad comment if it is already linked from issue metadata or cross reference
    4. Add a short `### Confusions` section at the bottom when any part of task execution was unclear
    5. Do not post any additional completion summary comment
11. Before moving to `HumanReview`, poll PR feedback and checks:
    1. Read any manual QA or reviewer validation notes and use them to sharpen runtime coverage
    2. Run the full PR feedback sweep protocol
    3. Confirm PR checks are passing after the latest changes
    4. Confirm every required issue provided validation item is explicitly marked complete in the workpad
    5. Repeat this check, address, and verify loop until no outstanding comments remain and checks are fully passing
    6. Re open and refresh the workpad before state transition so `Plan`, `Acceptance Criteria`, and `Validation` exactly match completed work
12. Only then move issue to `HumanReview`.
    1. Exception: if blocked by missing required non GitHub tools or auth, move to `HumanReview` with the blocker brief and explicit unblock actions
13. For `Todo` issues that already had a PR attached at kickoff:
    1. Ensure all existing PR feedback was reviewed and resolved, including inline review comments
    2. Ensure branch was pushed with any required updates
    3. Then move to `HumanReview`

## Step 3: Human review and merge handling

1. When the issue is in `HumanReview`, do not code or change issue content.
2. Poll for updates as needed, including GitHub PR review comments from humans and bots.
3. If review feedback requires changes, move the issue to `Rework` and follow the rework flow.
4. If approved, move the issue to `Merging`.
5. When the issue is in `Merging`, open and follow `.github/skills/land/SKILL.md`, then run the land skill in a loop until the PR is merged. Do not call `gh pr merge` directly unless the land skill requires it.
6. After merge is complete, move the issue to `Done`.

## Step 4: Rework handling

1. Treat `Rework` as a full approach reset, not incremental patching.
2. Re read the full issue body and all human comments. Explicitly identify what will be done differently this attempt.
3. Close the existing PR tied to the issue.
4. Remove the existing `## Rusty Workpad` comment from the issue.
5. Create a fresh branch from `origin/main`.
6. Start over from the normal kickoff flow:
   1. If current issue state is `Todo`, move it to `InProgress`. Otherwise keep the current state.
   2. Create a new bootstrap `## Rusty Workpad` comment.
   3. Build a fresh plan and checklist and execute end to end.

## Completion bar before human review

1. Step 1 and Step 2 checklist is fully complete and accurately reflected in the single workpad comment.
2. Acceptance criteria and required issue provided validation items are complete.
3. Validation and tests are green for the latest commit.
4. PR feedback sweep is complete and no actionable comments remain.
5. PR checks are green, branch is pushed, and PR is linked on the issue.
6. Required PR metadata is present, including the `rusty` label.
7. If app touching, runtime validation and media requirements are complete.

## Guardrails

1. If the branch PR is already closed or merged, do not reuse that branch or prior implementation state for continuation.
2. For closed or merged branch PRs, create a new branch from `origin/main` and restart from reproduction and planning as if starting fresh.
3. If issue state is `Backlog`, do not modify it. Wait for human to move to `Todo`.
4. Do not edit the issue body for planning or progress tracking.
5. Use exactly one persistent workpad comment, `## Rusty Workpad`, per issue.
6. If comment editing is unavailable in session, use `gh api` or `gh issue comment` based fallback scripts. Only report blocked if both direct editing and script based editing are unavailable.
7. Temporary proof edits are allowed only for local verification and must be reverted before commit.
8. If out of scope improvements are found, create a separate GitHub issue rather than expanding current scope, and include a clear title, description, acceptance criteria, and a reference back to the current issue.
9. Do not move to `HumanReview` unless the completion bar is satisfied.
10. In `HumanReview`, do not make changes. Wait and poll.
11. If state is terminal, do nothing and shut down.
12. Keep issue text concise, specific, and reviewer oriented.
13. If blocked and no workpad exists yet, add one blocker comment describing blocker, impact, and next unblock action.

## Suggested GitHub label set

Use these labels if they do not already exist:

1. `Backlog`
2. `Todo`
3. `InProgress`
4. `HumanReview`
5. `Merging`
6. `Rework`
7. `Done`
8. `Blocked`
9. `rusty`

## Suggested GitHub CLI operations

Use these commands or their API equivalents as the default control plane.

### Reading

1. Read issue\
   `gh issue view <number> --repo ridermw/rusty --json number,title,body,state,labels,comments,url`

2. Read PR reviews\
   `gh pr view <pr> --repo ridermw/rusty --json reviews,comments,labels,commits,statusCheckRollup`

3. Read inline review comments\
   `gh api repos/ridermw/rusty/pulls/<pr>/comments`

### State transitions (use Project status, not labels)

4. Get the project item ID for an issue\
   `gh project item-list 5 --owner ridermw --format json | jq '.items[] | select(.content.number == <number>)'`

5. Set status to InProgress\
   `gh project item-edit --project-id PVT_kwHOAFl5zs4BRqBN --id <ITEM_ID> --field-id PVTSSF_lAHOAFl5zs4BRqBNzg_a8jM --single-select-option-id 47fc9ee4`

6. Status option IDs for quick reference:
   - Backlog: `f75ad846`
   - Todo: `61e4505c`
   - InProgress: `47fc9ee4`
   - HumanReview: `df73e18b`
   - Merging: `37ec4a6e`
   - Rework: `84ea7dca`
   - Done: `98236657`

### PRs

7. Create PR\
   `gh pr create --repo ridermw/rusty --fill`

8. Add PR label\
   `gh pr edit <pr> --repo ridermw/rusty --add-label rusty`

## Workpad template

Use this exact structure for the persistent workpad comment and keep it updated in place throughout execution:

````md
## Rusty Workpad

```text
<hostname>:<abs_path>@<short_sha>
```

### Plan

- [ ] 1. Parent task
  - [ ] 1.1 Child task
  - [ ] 1.2 Child task
- [ ] 2. Parent task

### Acceptance Criteria

- [ ] Criterion 1
- [ ] Criterion 2

### Validation

- [ ] targeted tests: `<command>`

### Notes

- <short progress note with timestamp>

### Confusions

- <only include when something was confusing during execution>
````