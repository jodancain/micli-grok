# micli (this fork)

This repository is the **micli** product fork of [xai-org/grok-build](https://github.com/xai-org/grok-build)
(GitHub: [jodancain/micli-grok](https://github.com/jodancain/micli-grok)).

It keeps the full Grok TUI/UX and agent runtime, and routes inference through
**9route / ezr** (`https://router.yundongyl.cn/v1`) with a Cursor-Auto-style
**`auto`** model that picks `ezr/*` by **capability** (not a tiny fixed list).

Catalog source of truth for the 74 green models: project doc `9route-ezr-models.md`
(or your vault copy). Wire IDs always use the `ezr/` prefix.

## Quick setup (Mac)

1. Export your relay key (never commit it):

   ```sh
   export EZR_CLIENT_KEY="…"
   export XAI_API_KEY="$EZR_CLIENT_KEY"   # optional alias
   ```

2. Merge the preset into `~/.grok/config.toml`:

   ```sh
   # from this repo — covers every model used in Auto capability pools
   cat config/micli-9route.example.toml >> ~/.grok/config.toml
   ```

3. Build and install:

   ```sh
   cargo build -p xai-grok-pager-bin --release
   cp target/release/xai-grok-pager ~/.grok/bin/grok
   ```

4. Smoke test:

   ```sh
   grok -m ezr-claude-haiku-4-5 -p "ping"
   grok -m auto -p "refactor the auth module"
   # stderr: [micli-auto] capability=coding_agent model=ezr/claude-sonnet-5
   ```

## Auto capabilities

When the selected model is `auto` / `micli-auto` (or `[models] default = "auto"`),
each turn classifies the latest human prompt and remaps the **wire** model to an
`ezr/<name>` from that capability’s pool. The picker still shows Auto.

Force a capability: `MICLI_AUTO_CAPABILITY=vision grok -m auto -p "…"`
(legacy alias: `MICLI_AUTO_TIER`).

### Pools (primary → in-pool failover → adjacent pool)

| Capability | Intent signals | Primary |
|---|---|---|
| `coding_agent` | refactor / implement / debug / tool loops / 重构 / 实现 | `ezr/claude-sonnet-5` |
| `coding_fast` | small fix / lint / explain snippet | `ezr/claude-haiku-4-5` |
| `reasoning_heavy` | architecture / hard plan / proofs / 设计 / 架构 | `ezr/claude-opus-5` |
| `chat_default` | general Q&A | `ezr/claude-sonnet-5` |
| `cheap_bulk` | summarize / translate / classify / 总结 / 翻译 | `ezr/gpt-5.4-nano` |
| `vision` | screenshot / image / 图片 / 截图 | `ezr/glm-5v-turbo` |
| `long_context` | 大文档 / multi-file / research / long prompts | `ezr/gemini-3.1-pro-preview` |
| `chinese_biz` | 公文 / 飞书 / 企微 / 本体 / 业务文档 | `ezr/qwen3.8-max` |
| `creative_write` | fable / narrative / 小说 / 寓言 | `ezr/claude-fable-5-1` |

Full pool membership (all `ezr/…` IDs) is defined in
`crates/codegen/xai-grok-shell/src/micli_auto.rs` (`capability_pool`) and listed
by `format_capabilities_listing()` (unit-tested). Adjacent spillover examples:
`coding_agent` → `coding_fast`; `vision` → `chat_default`; `chinese_biz` → `chat_default`.

### coding_agent pool (example)

1. `ezr/claude-sonnet-5` (primary)
2. `ezr/kimi-k2.7-code`
3. `ezr/kimi-k2.7-code-highspeed`
4. `ezr/claude-sonnet-4-6`
5. `ezr/gpt-5.5`
6. `ezr/qwen3.8-max`
7. `ezr/deepseek-v4-pro`
8. `ezr/glm-5.3`
9. `ezr/grok-4.6`
10. …then adjacent `coding_fast` models not already listed

### Catalog / relay notes

- Wire model ids **must** be `ezr/<name>`.
- Relay `GET /v1/models` only lists ~4 models — rely on explicit `[model.*]`
  (see `config/micli-9route.example.toml`, ~35 pool models) + built-in Auto stub.
- GPT / `e-g-*` family: set `max_completion_tokens` (example toml does this).
- Auth: `EZR_CLIENT_KEY` (Bearer). Auto stub also accepts `XAI_API_KEY`.

### Listing capabilities

There is no separate TypeScript wrapper CLI required. From docs or tests:

```text
# In Rust (tests / future slash hook):
crate::micli_auto::format_capabilities_listing()
```

Or read this section + `micli_auto.rs`. A `grok` slash/CLI surface for
`auto capabilities` can be wired later; the listing API already exists.

### 429 / quota failover

When the selected catalog model is Auto and sampling fails with RateLimited / HTTP 429 / quota-like messages, micli advances `next_fallback` within the capability pool (then adjacent pool), refreshes sampler + `ConversationRequest.model`, and resubmits the turn (`CompactAndResubmit`). Logs: `[micli-auto] failover from → to`.

Wire-model sync: after remap, `run_turn_via_sampler` copies the remapped `ezr/...` id onto `ConversationRequest.model` so the HTTP body does not keep `model=auto`.

