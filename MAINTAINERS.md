# Maintainers

This document defines the review and merge policy of the mashfromband fork. The 2026-10-12 owner directive below replaces general human-maintainer approval and operational handoff requirements in this fork. Upstream roster and historical governance entries below are attribution/reference only; they do not describe this fork's permissions or require an upstream person's approval.

## Upstream maintainers (reference only)

| GitHub account | Project role | Responsibilities |
| --- | --- | --- |
| [@lidge-jun](https://github.com/lidge-jun) | Project owner | Project direction, releases, repository administration, and final governance decisions |
| [@Ingwannu](https://github.com/Ingwannu) | Maintainer | Issue and pull-request triage, `dev` integration, security review, and repository maintenance |

The table describes project responsibilities. Actual repository permissions remain controlled
through GitHub repository settings.

`dev` is the only integration line. The former `dev2-go` carry duty is retired;
see [The retired `dev2-go` line](#the-retired-dev2-go-line).

## Former upstream maintainers (history)

| GitHub account | Project role | Period |
| --- | --- | --- |
| [@Wibias](https://github.com/Wibias) | Maintainer | 2026-07-27 – 2026-08-19 |

Former maintainers keep contributor standing and are welcome to open issues and pull requests like
anyone else. Authorship credit in git history, release notes, and code comments is not rewritten
when a maintainer steps down.

## Review and merge policy

- Pull requests target `dev`. It is the only integration line, and promotion to
  `main` happens only from `dev`. The target-branch check accepts `dev` alone.
- The **`enforce-target`** CI check rejects pull requests whose head
  ancestry sits on the **`main`** tip while far behind **`dev`**, and rejects
  empty, thin, or malformed descriptions; PRs whose title or description
  mentions `gui` must include a screenshot of the UI change in the description.
  Contributor PRs (authors without repository push permission) open in draft
  and stay there until a four-box review-readiness checklist in the
  description is complete: local CI green, branch on the latest `dev` commit,
  all correct Codex and CodeRabbit findings fixed, and the ready-for-review
  confirmation. When all four boxes are ticked the gate marks the PR ready and
  notifies the maintainers listed in `MAINTAINERS.md` (excluding the author).
  Completion is bound to the exact commit the PR head pointed at: if new
  commits are pushed afterwards, the gate moves the PR back to draft, resets
  the checklist and the notification, and asks the author to test and tick the
  boxes again against the latest code.
  Before a completion is accepted, the gate verifies the checklist claims
  it can check itself: the branch must be on the latest `dev` commit or at
  most 10 commits behind it, and Codex/CodeRabbit findings must be resolved.
  The local-CI box is an author attestation only — fork contributors cannot
  start repository CI; a maintainer has to — so the gate never disproves it;
  a new push still resets every box. A disproved claim unticks the matching
  box and keeps the PR a draft.
  Authors with repository push permission skip the ancestry heuristic only. As
  with the approval requirement above, this part is enforced by convention;
  the ruleset does not check ancestry (see the note under the change log).
- In this mashfromband fork, AI assignees own design, implementation, checks, fixes, independent model review, resolution of Must findings, successful required CI for the exact final head, merge, necessary existing-production reflection and cleanup. They do not wait for a human maintainer's review or merge.
- Authors do not submit approving GitHub reviews of their own work. Independent model review is recorded with the fixed head and evidence; an author attestation alone is not independent review. Outstanding substantiated review findings must be resolved.
- Integration still targets `dev`; release and preview promotion retain their technical, security and verification gates. The AI assignee performs authorized promotion instead of handing it to a human. New publication, costs, rights or unspecified product requirements remain owner-only decisions under AGENTS.md. Upstream promotion and maintainer requirements apply only upstream.
- Authentication, credential handling, GitHub Actions, release automation, dependency installation,
  and other security-boundary changes require explicit security review.
- A new or promoted provider preset is a credential-destination change. Before merge it needs the
  primary-source evidence listed under [Adding a provider to the
  catalog](https://opencodex.me/contributing/#evidence-required-for-a-canonical-preset): documented
  OpenAI-compatible endpoints (including authenticated `GET /v1/models` when the entry declares
  `liveModels`), terms of service and operating legal entity, resale or routing authorization for
  aggregators, a named maintenance owner, and a citable verification date. Contributor affiliation
  with the service is disclosed, not disqualifying, and it does not lower the evidence bar. When the
  evidence is incomplete, prefer an inert `src/providers/free-directory.ts` reference row over a
  canonical registry entry.
- Security-sensitive and release-related changes should receive an additional independent security review when
  practical.
- Integration uses pull requests, including urgent maintainer repairs. The PR-only ruleset
  bypass does not authorize direct pushes; incident changes to branch protection require a
  separate owner decision.
- Promotion from `dev` to `main` is performed by the AI assignee after required verification. npm publication requires existing owner authorization; this policy creates no new publication permission.
- **Opening a release starts by moving `dev`'s version line forward.** Before cutting
  a release, `dev` must already outrank the version being released; `release.yml`
  asserts this and refuses to publish otherwise. Dispatch
  `.github/workflows/dev-version-bump.yml` with the intended version, merge the pull
  request it opens, then promote and release. When `dev` already outranks the target
  — a preview cut, or a stable hotfix below `dev`'s line — no move is needed and the
  workflow reports `changed=false`.

  Opening a preview for the next core ends the current patch line. After
  `vX.Y.0-preview.*` is tagged, a fix ships as part of `X.Y.0`, not as
  `X.(Y-1).(Z+1)`. The release helper refuses such a bump rather than producing a
  version the repository would reject. This is a deliberate policy restriction, not
  a claim that lower stable patches were historically unused.

  Done after the publish, as this repository did for ten releases (`32529c2b2`,
  `e4a85d134`, `076ad3036`, `befcac3e1`, then #3045, #3076, #3127, #3265, #3354,
  #3434), it leaves `dev` and every open pull request carrying a failure contributors
  cannot fix from their own diff. The pull request itself does not go away — `Protect
  dev` requires a reviewed merge. If the pre-move is missed and publication somehow
  succeeds, dispatch `dev-version-bump.yml` from the default branch with the released
  version and `mode=repair`, then merge the repair pull request. Design:
  `devlog/_plan/260904_release_version_line/`.

## The retired `dev2-go` line

`dev2-go` was a parallel integration line that rebuilt the runtime as a Go
native port, and policy required every merge into `dev` to be rebased onto it
and ported under `go/`. That policy is withdrawn as of 2026-07-30.

The dual-track cost outran its return: the carry backlog never cleared (17
commits and 9 open `needs-go-port` issues at the time of the decision, against
594 commits of divergence), and dogfooding the Go runtime kept producing new
defects. Bun-native TypeScript on `dev` is the single runtime line again.

- The branch has been deleted from this repository. Its full history is
  published at
  [lidge-jun/opencodex-go-archive](https://github.com/lidge-jun/opencodex-go-archive),
  and its final tip stays reachable here as the `archive/dev2-go` tag.
- A merge into `dev` carries no port obligation. The nine open `needs-go-port`
  issues (#661, #663, #666, #670, #674, #678, #680, #685, #703) were closed as
  not planned, and the `needs-go-port` label no longer exists on the
  repository.
- Future native work is expected to be an incremental module landing on `dev`
  (Rust via N-API is the current candidate), not a second integration branch.
  Reopening a parallel runtime line is an owner decision.

## Upstream maintainer changes (reference and history only)

Adding or removing a maintainer requires:

1. agreement from the project owner,
2. review by another current maintainer when available, and
3. updates to this file and [`.github/CODEOWNERS`](./.github/CODEOWNERS).

### Change log

- 2026-09-06 — The owner authorized explicit maintainer integration into `dev` without a second
  maintainer approval. Both current maintainers have `admin` access. The dev-only PR bypass
  includes GitHub's `admin` and `maintain` roles; `write` access alone is insufficient. Contributor
  review remains the default and the `main`/`preview` rules are unchanged. The optional
  `scripts/ci/assert-mergeable-review.sh --maintainer-integration <pr-number> [repo]` path checks
  the authenticated actor against the trusted `dev` roster and live repository permissions,
  preserves outstanding maintainer objections, and binds its result to the current head and base.
  The helper emits a validation snapshot, not a ready-to-run privileged merge command: head
  matching does not pin a PR's base, which may change after inspection. Revalidate the current
  actor and `dev` base before a separately authorized merge. The helper is not proof of CI or
  security review and not a barrier against an administrator bypassing it. Repository settings
  remain authoritative for actual permissions.

- 2026-08-19 — [@Wibias](https://github.com/Wibias) stepped down as a maintainer
  and is now a contributor. This follows his own decision to stop developing
  opencodex; it is not a disciplinary action, and it was made with the owner's
  agreement (requirement 1). Requirement 2 does not apply to a maintainer's own
  resignation, which needs no second maintainer to ratify it. Requirement 3 is
  met by this file and `.github/CODEOWNERS`, where the default-reviewer line
  and the four runtime paths that listed him (`/src/adapters/`,
  `/src/providers/`, `/src/codex/`, `/src/server/`) drop back to the two
  remaining maintainers. Repository permission was reduced to read access at
  the same time, so the roster and the GitHub settings agree again.

  Nothing he authored is being unwound. His commits, the pull requests he
  merged, the release-note attributions, and the code comments citing his
  reviews stay exactly as they are, and the trust-lane gate derived from his
  work in `.github/scripts/pr-sponsored-surface.cjs` keeps its attribution.
  Returning to the maintainer table later would go through the same three
  requirements that govern every addition.

- 2026-07-27 — [@Wibias](https://github.com/Wibias) added as a maintainer.
  Requirement 1 (agreement from the project owner) is met: the owner requested
  the addition. **Requirement 2 (review by another current maintainer) was
  never satisfied in the form this document describes.** The three commits that
  carried the addition (`a2693c02`, `dc3a4ade`, `02bbd47a`) landed on `dev` as
  direct owner pushes with no associated pull request, so no second maintainer
  reviewed them. Requirement 3 is met by this file and `.github/CODEOWNERS`.
  The addition took effect regardless: @Wibias held write access on the
  repository and merged pull requests from 2026-07-26 until he stepped down on
  2026-08-19. This entry records the gap rather than papering over it — a later
  maintainer change should go through a reviewed pull request.

  Scope covers issue and pull-request triage, `dev` integration, and
  provider/CI maintenance. (This entry originally also described carrying
  merged `dev` work onto `dev2-go`; that duty ended when the line was retired
  on 2026-07-30.) Security-boundary ownership in `.github/CODEOWNERS` is
  deliberately unchanged: authentication, credential handling, GitHub Actions,
  and release automation keep the two owners already listed for those paths, so
  this addition does not widen the review surface for them.

  Code-owner approval and the maintainer-approval requirement above are both
  enforced, not conventions. `dev`, `main`, and `preview` each carry an active
  repository ruleset — the classic `/branches/{branch}/protection` endpoint
  returns 404 for them, which is why this file long described the repository as
  unprotected. `Protect dev` (id 20763889) requires a pull request with one
  approving review, code-owner review, and extra approval for unattributed
  changes, and it blocks deletion and non-fast-forward pushes. Allowed merge
  methods are merge and squash; rebase merges are off.

  At that time, the actual PR bypass covered `admin`; the earlier wording that
  included `maintain` was inaccurate. The 2026-09-06 policy above adds the explicit
  maintainer-integration exception for `dev` and the corresponding `maintain` role.
  Both roles bypass through pull requests only. Force-push and deletion protections
  remain in place, and the integrating maintainer records the decision and evidence.

## Security reports

Private vulnerability reports are handled by the current maintainers according to
[`SECURITY.md`](./SECURITY.md). Do not disclose secrets or exploit details in a public issue.

## この所有者の開発運用（2026-10-12）

この規則は GitHub の実所有者が mashfromband の場合に適用する。オーナーの2026-10-12の直接指示（共有ToDo2458/2465）が、古い一般的な人手レビュー・マージ・日常操作・再承認の前提に優先する。他の所有者や顧客固有の契約には適用しない。

人は要件を決め、本人だけが決められる質問に答える。AI担当は設計・実装・検査・失敗の修正・別モデルレビュー・Must解消・同じ最終HEADのCI確認・PRマージ・必要な既存Prdへの反映・後片付けまで行う。PRを作っただけ、委譲を受け付けただけでは完了にしない。日常操作や技術的な選択について、人のレビューや再承認を待たない。

既存のDiscord回答は元ToDo・質問・本人ID・対象・範囲と照合し、同じ判断を再質問しない。未指定の製品要件、新費用、権利、顧客との新しい約束、公開、取り消せない操作など本人だけが決める点は、PCのSkill discord-owner-askに従って具体的な選択肢を1回送る。その項目だけ共有ToDoのblockにし、独立した作業を続ける。他ToDoの完了待ちはwait --onで記録する。

プロジェクト固有の検査・受入条件・秘密保護・顧客固有の人によるレビュー要求・既存Prd限定の制約は維持する。新費用・新APIキーは使わない。Vercel/Railwayのオーナーアカウント内はソロ運用とし、他メンバーがいることだけで承認待ちにしない。他OrgのGitHub環境は変更しない。ドメイン管理はRakkoだけを使う。コマンドは窓を出さず担当が実行する。

既存checkoutでは実remoteとGitHub ownerを再照合し、読み書き前にscrub391 --guard-pathで保護判定する。保護対象には触らず名前・パスを報告しない。専用branchで作業し、他担当の未コミットを巻き込まない。稼働中担当を止めず、元要件を保持した保存specまたは正式配送で次の区切りから反映させる。受付・読取・実反映を区別する。

完了時は変更・確認済み・未確認・残作業を件数で記録し、PR URL、固定HEAD、検査、CI、別モデルレビュー、マージ、Prd反映の結果を元の共有ToDoへ残す。一時データと自作ChromeタブはPCの所有記録に従って片付ける。
