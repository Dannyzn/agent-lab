# ADR 0002: macOS execution boundary

Status: Implemented in Phase 2, 2026-11-01.

Use one host-owned policy boundary before tool dispatch. Read/search are allowed; patch/exec require per-call approval by default. Approval grants one immutable call and cannot widen capabilities. Hard policy denial precedes approval.

Use a dedicated `agent-sandbox` crate with macOS Seatbelt as the default execution backend. Unsupported environments fail closed; an explicit `--sandbox none` is available for development and warns that OS file/network enforcement is absent. Linux enforcement is deferred.

Tool subprocess capabilities are read-only or workspace-write, with network deny or allow. Workspace access is supplemented by read-only system runtime paths and a private writable scratch directory. The host model client remains outside the tool sandbox. Environment inheritance is replaced with a minimal fixed environment, private HOME/TMPDIR and trusted executable search paths. Never forward provider credentials to children.

Use a sticky cancellation token for model sampling, approval and command execution. Commands get independent POSIX process groups; timeout, cancellation and parent exit clean the group. Buffer command output incrementally with bounded memory. Close active item/approval lifecycles before the cancellation terminal event. No resume is implied by public JSONL.

Known boundary: this is a local supervised coding runtime, not a hostile multi-tenant process supervisor. Process-group escape through setsid, hostile concurrent symlink/hardlink mutation, workspace-resident secrets, and system runtime roots need stronger containment or a VM/container before handling adversarial workloads. Synchronous file tools observe cancellation at operation boundaries; changes already committed are not rolled back.
