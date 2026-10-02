# ADR 0001: Agent harness, omp over RPC with host tools

- **Date:** 2026-10-02
- **Status:** Accepted, with release blockers
- **Context:** spec `docs/superpowers/specs/2026-10-02-folder-sources-grounded-answers-design.md` §7.1 (acceptance gate)

## Decision

Shodh drives oh-my-pi (`omp`) as a pinned sidecar over its JSONL RPC protocol. All capabilities are exposed as **host tools** registered with `set_host_tools` and executed in Rust.

Sub-agents are **not** spawned through omp's built-in `task` tool. Shodh implements a host-side `delegate` tool. It starts a separate omp session restricted to the child profile's host tools, and renders that session's events as a sub-agent lane.

## Spike setup

| Item | Value |
|---|---|
| Binary | omp v18.4.10, `omp-windows-x64.exe` |
| Checksum | SHA-256 verified against the release's `SHA256SUMS.txt` |
| Model | `openrouter/stealth/space-bunny-alpha` |
| Corpus | Synthetic: 5 business documents |
| Isolation | Isolated `HOME`/`USERPROFILE`; config overlay disabling every discovery provider |
| Bundled agents | All disabled |

Launch flags: `--mode rpc --no-ui --tools task --no-extensions --no-skills --no-rules --no-lsp --no-pty --no-session --thinking off`.

## Results

| Gate | Result | Evidence |
|---|---|---|
| G1: built-in tools disabled | Pass | `get_state.dumpTools` listed exactly `task` and `search_documents` |
| G2: host tool round-trip | Pass | `host_tool_call`, then `host_tool_update` and `host_tool_result`; correct answer (60-day notice) cited from the right file |
| G3: cancellation | Pass | `abort` mid-stream; `prompt_result.status = aborted`; settled in 7–8 ms |
| G4: usage accounting | Pass | Per-message usage (`input`, `output`, `cacheRead`, `cacheWrite`, `cost`) and `get_session_stats` totals |
| G6: sub-agent isolation | Pass, not usable | A sub-agent asked to run shell commands and write files had only `yield`. No files were created. **Host tools are not available to `task` children**, so they cannot do retrieval |
| G5: egress | **Fail** | Through a recording CONNECT proxy: `openrouter.ai` (expected), **`catalog.stencil.so`** (model catalog fetch, no documented off switch), and probes of `127.0.0.1:11434`, `:8080` and `:1234` (Ollama, llama.cpp and LM Studio discovery) |

Other observations:
- Custom agent definitions load from `~/.omp/agent/agents`, not from `PI_CODING_AGENT_DIR`.
- `task.disabledAgents` is enforced at preflight.
- `subagent_lifecycle`, `subagent_progress` and `subagent_event` frames arrive with `set_subagent_subscription`.

## Consequences

1. **`delegate` is host-side.** The child session gets exactly the child profile's host tools, with permissions intersected with the parent's. Lanes are rendered from Shodh's own events, so lane rendering does not depend on omp's undocumented sub-agent frame fields.
2. **`task` is disabled.** The production harness launches with `--no-tools`, not `--tools task`.
3. **Egress is a release blocker.**
   - The proxy test is a lower bound: traffic that ignores `HTTPS_PROXY` bypasses it.
   - Release requires OS-level egress control for the omp process, allowing only the configured LLM endpoints. Options are a Windows Filtering Platform rule per executable, or a sandbox.
   - Release also requires one of: a patched pinned build that removes the catalog fetch and local-port discovery, or an upstream configuration flag if one is added.
   - The spike's proxy test stays in CI as a regression check. It is not the release gate.
4. **Pinning and contract tests are mandatory.** The binary is pinned by hash. Every RPC frame shape Shodh relies on gets a contract test, re-run on each upgrade.
5. **Stealth models are disallowed.** OpenRouter stealth models typically log prompts for training, so they are never permitted with user documents. The provider policy must support an allow-list.

## Alternatives considered

- **Upstream pi:** safer governance, but no host-tool sub-protocol, no permission tiers, and no in-band tool dialects for local models.
- **Hermes:** a whole product, requires a Python 3.14 runtime, no text tool-call fallback, and a single trust tier.
- **In-process Rust loop:** remains the fallback behind `AgentHarness` if the egress blockers cannot be closed.
