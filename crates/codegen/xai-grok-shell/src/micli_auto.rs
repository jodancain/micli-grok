//! micli **Auto** model router for 9route/ezr.
//!
//! Capability / function based routing across the 74-model ezr catalog
//! (see `/workspace/9route-ezr-models.md` / project docs). When the selected
//! catalog model is `auto` / `micli-auto`, classify the user prompt into a
//! capability pool and map to a concrete `ezr/<name>` API id before the
//! inference call.
//!
//! Failover: [`next_fallback`] advances within the pool, then into an adjacent
//! pool. Mid-turn 429 wiring in the sampler loop is a follow-up (see `MICLI.md`).

use std::collections::HashMap;
use std::sync::OnceLock;

/// Capability pool for Auto model selection (Cursor-Auto style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoCapability {
    /// Edit / refactor / debug / implement / tool loops.
    CodingAgent,
    /// Small fix, explain snippet, lint.
    CodingFast,
    /// Architecture, hard bugs, multi-step plan, proofs.
    ReasoningHeavy,
    /// General Q&A.
    ChatDefault,
    /// Summarize, classify, rewrite, translate, extract.
    CheapBulk,
    /// Image / screenshot / 图.
    Vision,
    /// Large docs, multi-file梳理, research.
    LongContext,
    /// 中文公文 / 飞书 / 企微 / 本体 / 业务文档.
    ChineseBiz,
    /// Fable / narrative / creative writing.
    CreativeWrite,
}

impl AutoCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodingAgent => "coding_agent",
            Self::CodingFast => "coding_fast",
            Self::ReasoningHeavy => "reasoning_heavy",
            Self::ChatDefault => "chat_default",
            Self::CheapBulk => "cheap_bulk",
            Self::Vision => "vision",
            Self::LongContext => "long_context",
            Self::ChineseBiz => "chinese_biz",
            Self::CreativeWrite => "creative_write",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "coding_agent" | "coding" | "agent" => Some(Self::CodingAgent),
            "coding_fast" | "fast" => Some(Self::CodingFast),
            "reasoning_heavy" | "heavy" | "reasoning" => Some(Self::ReasoningHeavy),
            "chat_default" | "default" | "chat" => Some(Self::ChatDefault),
            "cheap_bulk" | "cheap" | "bulk" => Some(Self::CheapBulk),
            "vision" => Some(Self::Vision),
            "long_context" | "research" | "long" => Some(Self::LongContext),
            "chinese_biz" | "chinese" | "biz" => Some(Self::ChineseBiz),
            "creative_write" | "creative" | "fable" | "write" => Some(Self::CreativeWrite),
            // legacy tier aliases from the first micli Auto sketch
            _ => None,
        }
    }

    pub fn all() -> &'static [AutoCapability] {
        &[
            Self::CodingAgent,
            Self::CodingFast,
            Self::ReasoningHeavy,
            Self::ChatDefault,
            Self::CheapBulk,
            Self::Vision,
            Self::LongContext,
            Self::ChineseBiz,
            Self::CreativeWrite,
        ]
    }

    /// Human blurb for docs / `capabilities` listing.
    pub fn description(self) -> &'static str {
        match self {
            Self::CodingAgent => "edit / refactor / debug / implement / tool loops",
            Self::CodingFast => "small fix, explain snippet, lint",
            Self::ReasoningHeavy => "architecture, hard bugs, multi-step plan, proofs",
            Self::ChatDefault => "general Q&A",
            Self::CheapBulk => "summarize, classify, rewrite, translate, extract",
            Self::Vision => "image / screenshot / 图",
            Self::LongContext => "large docs, multi-file梳理, research",
            Self::ChineseBiz => "中文公文 / 飞书 / 企微 / 本体 / 业务文档",
            Self::CreativeWrite => "fable / narrative / creative writing",
        }
    }
}

impl std::fmt::Display for AutoCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Backward-compatible alias used by early call sites / docs.
pub type AutoTier = AutoCapability;

/// One resolved Auto pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoResolution {
    pub capability: AutoCapability,
    /// Sent on the wire to 9route (e.g. `ezr/claude-sonnet-5`).
    pub model_api_id: String,
    /// Local `[model.<key>]` / catalog key (e.g. `ezr-claude-sonnet-5`).
    pub catalog_key: String,
    /// Index into the flattened primary+failover(+adjacent) chain.
    pub chain_index: usize,
}

impl AutoResolution {
    /// Legacy field name used in early logs/tests.
    pub fn tier(&self) -> AutoCapability {
        self.capability
    }
}


/// Per-session Auto pick for the current user prompt (survives mid-turn failover).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoTurnState {
    pub prompt: String,
    pub resolution: AutoResolution,
}

fn auto_turn_map() -> &'static std::sync::Mutex<HashMap<String, AutoTurnState>> {
    static MAP: OnceLock<std::sync::Mutex<HashMap<String, AutoTurnState>>> = OnceLock::new();
    MAP.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Resolve for this session+prompt, reusing a prior failover index when the prompt is unchanged.
pub fn resolution_for_session(
    session_id: &str,
    prompt: &str,
    forced_capability: Option<AutoCapability>,
) -> AutoResolution {
    let mut map = auto_turn_map().lock().expect("micli auto turn map");
    if let Some(st) = map.get(session_id) {
        if st.prompt == prompt {
            return st.resolution.clone();
        }
    }
    let resolution = resolve_auto_model(prompt, forced_capability);
    map.insert(
        session_id.to_string(),
        AutoTurnState {
            prompt: prompt.to_string(),
            resolution: resolution.clone(),
        },
    );
    resolution
}

/// Advance this session's Auto chain after a quota/429 failure. Returns the new resolution.
pub fn advance_failover(session_id: &str) -> Option<AutoResolution> {
    let mut map = auto_turn_map().lock().expect("micli auto turn map");
    let st = map.get_mut(session_id)?;
    let next = next_fallback(&st.resolution)?;
    st.resolution = next.clone();
    Some(next)
}

/// Current Auto resolution for a session, if any.
pub fn current_resolution(session_id: &str) -> Option<AutoResolution> {
    auto_turn_map()
        .lock()
        .expect("micli auto turn map")
        .get(session_id)
        .map(|s| s.resolution.clone())
}

/// Whether `id` is the synthetic Auto catalog key / placeholder API id.
pub fn is_auto_model_id(id: &str) -> bool {
    matches!(
        id.trim().to_ascii_lowercase().as_str(),
        "auto" | "micli-auto"
    )
}

/// Map an `ezr/<name>` API id to the conventional local catalog key `ezr-<name>`.
pub fn catalog_key_for_api_id(api_id: &str) -> String {
    if let Some(rest) = api_id.strip_prefix("ezr/") {
        format!("ezr-{rest}")
    } else {
        api_id.replace('/', "-")
    }
}

/// True when the bare model name (after `ezr/`) is GPT / e-g-* and needs
/// `max_completion_tokens` on the wire.
pub fn needs_max_completion_tokens(api_id: &str) -> bool {
    let name = api_id.strip_prefix("ezr/").unwrap_or(api_id);
    name.starts_with("gpt-") || name.starts_with("e-g-")
}

/// Primary + in-pool failover API ids for each capability (all `ezr/...`).
pub fn capability_pool(cap: AutoCapability) -> &'static [&'static str] {
    match cap {
        AutoCapability::CodingAgent => &[
            "ezr/claude-sonnet-5",
            "ezr/kimi-k2.7-code",
            "ezr/kimi-k2.7-code-highspeed",
            "ezr/claude-sonnet-4-6",
            "ezr/gpt-5.5",
            "ezr/qwen3.8-max",
            "ezr/deepseek-v4-pro",
            "ezr/glm-5.3",
            "ezr/grok-4.6",
        ],
        AutoCapability::CodingFast => &[
            "ezr/claude-haiku-4-5",
            "ezr/kimi-k2.7-code-highspeed",
            "ezr/gemini-3.8-flash",
            "ezr/gpt-5.4-mini",
            "ezr/deepseek-v4.1-flash",
            "ezr/glm-5.3-flash",
        ],
        AutoCapability::ReasoningHeavy => &[
            "ezr/claude-opus-5",
            "ezr/gpt-5.6-sol",
            "ezr/claude-opus-4-8",
            "ezr/gpt-6-astra",
            "ezr/gemini-3.1-pro-preview",
            "ezr/qwen3.8-max",
            "ezr/deepseek-v4-pro",
        ],
        AutoCapability::ChatDefault => &[
            "ezr/claude-sonnet-5",
            "ezr/gpt-5.5",
            "ezr/gemini-3.1-pro-preview",
            "ezr/grok-4.6",
            "ezr/glm-5.2",
            "ezr/MiniMax-M3",
        ],
        AutoCapability::CheapBulk => &[
            "ezr/gpt-5.4-nano",
            "ezr/claude-haiku-4-5",
            "ezr/gemini-3.5-flash-lite",
            "ezr/deepseek-flash",
            "ezr/glm-5.3-flash",
            "ezr/seed-2-0-lite-260228",
            "ezr/gpt-4o-mini",
        ],
        AutoCapability::Vision => &[
            "ezr/glm-5v-turbo",
            "ezr/deepseek-v4-flash-vision-exp",
            "ezr/gpt-4o",
            "ezr/gemini-2.5-pro",
            "ezr/claude-sonnet-5",
        ],
        AutoCapability::LongContext => &[
            "ezr/gemini-3.1-pro-preview",
            "ezr/claude-opus-5",
            "ezr/qwen3.8-max",
            "ezr/kimi-k3",
            "ezr/gpt-5.6-terra",
        ],
        AutoCapability::ChineseBiz => &[
            "ezr/qwen3.8-max",
            "ezr/glm-5.3",
            "ezr/deepseek-v4-pro",
            "ezr/doubao-seed-2-0-pro-260215",
            "ezr/claude-sonnet-5",
        ],
        AutoCapability::CreativeWrite => &[
            "ezr/claude-fable-5-1",
            "ezr/claude-fable-5",
            "ezr/gpt-5.5",
            "ezr/MiniMax-M3",
        ],
    }
}

/// Adjacent capability when the home pool is exhausted (Cursor-like spillover).
pub fn adjacent_capability(cap: AutoCapability) -> Option<AutoCapability> {
    match cap {
        AutoCapability::CodingAgent => Some(AutoCapability::CodingFast),
        AutoCapability::CodingFast => Some(AutoCapability::CodingAgent),
        AutoCapability::ReasoningHeavy => Some(AutoCapability::CodingAgent),
        AutoCapability::ChatDefault => Some(AutoCapability::CheapBulk),
        AutoCapability::CheapBulk => Some(AutoCapability::ChatDefault),
        AutoCapability::Vision => Some(AutoCapability::ChatDefault),
        AutoCapability::LongContext => Some(AutoCapability::ReasoningHeavy),
        AutoCapability::ChineseBiz => Some(AutoCapability::ChatDefault),
        AutoCapability::CreativeWrite => Some(AutoCapability::ChatDefault),
    }
}

/// Flattened failover chain: home pool, then adjacent pool (deduped).
pub fn failover_chain(cap: AutoCapability) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = capability_pool(cap).to_vec();
    if let Some(adj) = adjacent_capability(cap) {
        for id in capability_pool(adj) {
            if !out.contains(id) {
                out.push(id);
            }
        }
    }
    out
}

/// Legacy name kept for early call sites.
pub fn tier_chain(cap: AutoCapability) -> Vec<&'static str> {
    failover_chain(cap)
}

fn resolution_at(capability: AutoCapability, chain_index: usize) -> AutoResolution {
    let chain = failover_chain(capability);
    let idx = chain_index.min(chain.len().saturating_sub(1));
    let model_api_id = chain[idx].to_string();
    let catalog_key = catalog_key_for_api_id(&model_api_id);
    AutoResolution {
        capability,
        model_api_id,
        catalog_key,
        chain_index: idx,
    }
}

/// Advance to the next model in the flattened failover chain, if any.
pub fn next_fallback(current: &AutoResolution) -> Option<AutoResolution> {
    let chain = failover_chain(current.capability);
    let next = current.chain_index.saturating_add(1);
    if next >= chain.len() {
        return None;
    }
    Some(resolution_at(current.capability, next))
}

/// Multi-signal heuristic classifier → capability.
///
/// Priority (first match wins): vision → creative_write → chinese_biz →
/// long_context → reasoning_heavy → coding_agent → coding_fast → cheap_bulk →
/// chat_default. Length (≥ ~6k chars or many file-path-like tokens) boosts
/// long_context when no stronger signal fired earlier.
pub fn classify_prompt(prompt: &str) -> AutoCapability {
    let lower = prompt.to_ascii_lowercase();
    let char_len = prompt.chars().count();

    const VISION: &[&str] = &[
        "screenshot",
        "image",
        "photo",
        "diagram",
        "picture",
        "vision",
        "ocr",
        "png",
        "jpg",
        "jpeg",
        "webp",
        "图片",
        "截图",
        "图像",
        "看图",
    ];
    if VISION.iter().any(|k| lower.contains(k)) {
        return AutoCapability::Vision;
    }

    const CREATIVE: &[&str] = &[
        "fable",
        "narrative",
        "short story",
        "write a story",
        "fairy tale",
        "novel chapter",
        "creative writing",
        "寓言",
        "小说",
        "故事",
        "叙事",
    ];
    if CREATIVE.iter().any(|k| lower.contains(k)) {
        return AutoCapability::CreativeWrite;
    }

    const CN_BIZ: &[&str] = &[
        "公文",
        "飞书",
        "企微",
        "企业微信",
        "钉钉",
        "本体",
        "业务文档",
        "规章制度",
        "请示",
        "批复",
        "通知公告",
        "会议纪要",
        "汇报材料",
    ];
    if CN_BIZ.iter().any(|k| lower.contains(k)) {
        return AutoCapability::ChineseBiz;
    }

    const LONG: &[&str] = &[
        "long context",
        "entire codebase",
        "whole repo",
        "multi-file",
        "many files",
        "across the codebase",
        "research the",
        "literature review",
        "大文档",
        "多文件",
        "梳理整个",
        "全文分析",
        "跨仓库",
    ];
    let pathish = lower.matches(".rs").count()
        + lower.matches(".ts").count()
        + lower.matches(".py").count()
        + lower.matches(".go").count()
        + lower.matches(".md").count();
    if LONG.iter().any(|k| lower.contains(k)) || char_len >= 6000 || pathish >= 8 {
        return AutoCapability::LongContext;
    }

    const HEAVY: &[&str] = &[
        "architecture",
        "design system",
        "deep dive",
        "tradeoff",
        "trade-off",
        "roadmap",
        "prove that",
        "proof of",
        "multi-step plan",
        "hard bug",
        "root cause analysis",
        "设计",
        "架构",
        "方案对比",
        "论证",
    ];
    let heavy_word = lower.split_whitespace().any(|w| {
        matches!(
            w.trim_matches(|c: char| !c.is_alphanumeric()),
            "plan" | "design" | "architect" | "analyse" | "analyze" | "proof"
        )
    });
    if heavy_word || HEAVY.iter().any(|k| lower.contains(k)) {
        return AutoCapability::ReasoningHeavy;
    }

    const AGENT: &[&str] = &[
        "refactor",
        "implement",
        "debug",
        "fix the bug",
        "fix this bug",
        "tool call",
        "agent loop",
        "edit the file",
        "apply the patch",
        "write the code",
        "pull request",
        "code review",
        "stacktrace",
        "stack trace",
        "compile error",
        "typeerror",
        "修复",
        "改bug",
        "重构",
        "实现",
        "调试",
    ];
    if AGENT.iter().any(|k| lower.contains(k)) {
        return AutoCapability::CodingAgent;
    }

    const FAST: &[&str] = &[
        "small fix",
        "quick fix",
        "typo",
        "lint",
        "explain this snippet",
        "explain the snippet",
        "what does this function",
        "one-liner",
        "one liner",
        "格式化",
        "解释这段代码",
    ];
    if FAST.iter().any(|k| lower.contains(k)) {
        return AutoCapability::CodingFast;
    }
    // Mild coding without agent weight → still coding_fast.
    const MILD_CODE: &[&str] = &["bug", "fix", "snippet", "function", "编译"];
    if MILD_CODE.iter().any(|k| lower.contains(k)) && char_len < 800 {
        return AutoCapability::CodingFast;
    }
    if MILD_CODE.iter().any(|k| lower.contains(k)) {
        return AutoCapability::CodingAgent;
    }

    const CHEAP: &[&str] = &[
        "summarize",
        "summary",
        "classify",
        "rewrite",
        "translate",
        "extract",
        "tl;dr",
        "tldr",
        "bullet points",
        "翻译",
        "总结",
        "摘要",
        "分类",
        "改写",
        "抽取",
    ];
    if CHEAP.iter().any(|k| lower.contains(k)) {
        return AutoCapability::CheapBulk;
    }

    AutoCapability::ChatDefault
}

/// Public API: resolve Auto → capability + `ezr/...` ids.
pub fn resolve_auto_model(
    prompt: &str,
    forced_capability: Option<AutoCapability>,
) -> AutoResolution {
    let capability = forced_capability.unwrap_or_else(|| classify_prompt(prompt));
    resolution_at(capability, 0)
}

/// Convenience matching the task signature shape:
/// `(capability, model_api_id, catalog_key)`.
pub fn resolve_auto_model_tuple(
    prompt: &str,
    forced_capability: Option<AutoCapability>,
) -> (AutoCapability, String, String) {
    let r = resolve_auto_model(prompt, forced_capability);
    (r.capability, r.model_api_id, r.catalog_key)
}

/// True when an error message looks like provider quota / rate-limit exhaustion.
pub fn is_quota_or_rate_limit_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("insufficient_quota")
        || lower.contains("insufficient quota")
        || lower.contains("rate_limit")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("quota exceeded")
        || lower.contains("429")
}

/// Human-readable listing of all capability pools (for docs / CLI help).
pub fn format_capabilities_listing() -> String {
    let mut out = String::from("micli Auto capabilities (ezr pools):\n");
    for cap in AutoCapability::all() {
        out.push_str(&format!(
            "\n## {} — {}\n",
            cap.as_str(),
            cap.description()
        ));
        for (i, id) in capability_pool(*cap).iter().enumerate() {
            let tag = if i == 0 { "primary" } else { "failover" };
            out.push_str(&format!("  - [{tag}] {id}\n"));
        }
        if let Some(adj) = adjacent_capability(*cap) {
            out.push_str(&format!(
                "  adjacent pool after exhaustion: {}\n",
                adj.as_str()
            ));
        }
    }
    out
}

/// Deduped set of every `ezr/...` id referenced by any capability pool
/// (for generating `[model.*]` presets).
pub fn all_pool_model_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = Vec::new();
    for cap in AutoCapability::all() {
        for id in capability_pool(*cap) {
            if !ids.contains(id) {
                ids.push(id);
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_auto_ids() {
        assert!(is_auto_model_id("auto"));
        assert!(is_auto_model_id("Auto"));
        assert!(is_auto_model_id(" micli-auto "));
        assert!(!is_auto_model_id("ezr/claude-sonnet-5"));
        assert!(!is_auto_model_id("grok-4.6"));
    }

    #[test]
    fn catalog_key_from_api_id() {
        assert_eq!(
            catalog_key_for_api_id("ezr/claude-sonnet-5"),
            "ezr-claude-sonnet-5"
        );
        assert_eq!(
            catalog_key_for_api_id("ezr/claude-sonnet-4-6"),
            "ezr-claude-sonnet-4-6"
        );
        assert_eq!(catalog_key_for_api_id("ezr/gpt-5.5"), "ezr-gpt-5.5");
    }

    #[test]
    fn gpt_family_needs_max_completion_tokens() {
        assert!(needs_max_completion_tokens("ezr/gpt-5.5"));
        assert!(needs_max_completion_tokens("ezr/e-g-5.5"));
        assert!(needs_max_completion_tokens("ezr/gpt-4o-mini"));
        assert!(!needs_max_completion_tokens("ezr/claude-sonnet-5"));
        assert!(!needs_max_completion_tokens("ezr/gemini-3.8-flash"));
    }

    #[test]
    fn classify_vision() {
        assert_eq!(
            classify_prompt("What's wrong in this screenshot?"),
            AutoCapability::Vision
        );
        assert_eq!(classify_prompt("描述这张图片"), AutoCapability::Vision);
    }

    #[test]
    fn classify_coding_agent() {
        assert_eq!(
            classify_prompt("Please refactor the auth module and implement retries"),
            AutoCapability::CodingAgent
        );
        assert_eq!(
            classify_prompt("帮忙重构并修复这个编译错误"),
            AutoCapability::CodingAgent
        );
    }

    #[test]
    fn classify_coding_fast() {
        assert_eq!(
            classify_prompt("small fix: typo in README"),
            AutoCapability::CodingFast
        );
        assert_eq!(
            classify_prompt("explain this snippet of rust"),
            AutoCapability::CodingFast
        );
    }

    #[test]
    fn classify_reasoning_heavy() {
        assert_eq!(
            classify_prompt("Plan the migration architecture"),
            AutoCapability::ReasoningHeavy
        );
        assert_eq!(
            classify_prompt("请设计系统架构并论证取舍"),
            AutoCapability::ReasoningHeavy
        );
    }

    #[test]
    fn classify_cheap_bulk() {
        assert_eq!(
            classify_prompt("translate this to English"),
            AutoCapability::CheapBulk
        );
        assert_eq!(
            classify_prompt("请总结一下这段文字"),
            AutoCapability::CheapBulk
        );
    }

    #[test]
    fn classify_chinese_biz() {
        assert_eq!(
            classify_prompt("帮我写一份飞书会议纪要公文"),
            AutoCapability::ChineseBiz
        );
        assert_eq!(
            classify_prompt("整理企微业务文档本体"),
            AutoCapability::ChineseBiz
        );
    }

    #[test]
    fn classify_creative() {
        assert_eq!(
            classify_prompt("Write a short fable about a fox"),
            AutoCapability::CreativeWrite
        );
    }

    #[test]
    fn classify_long_context() {
        assert_eq!(
            classify_prompt("请对这份大文档做多文件梳理 research the entire codebase"),
            AutoCapability::LongContext
        );
    }

    #[test]
    fn classify_chat_default() {
        assert_eq!(
            classify_prompt("Hello, how are you?"),
            AutoCapability::ChatDefault
        );
    }

    #[test]
    fn vision_beats_coding_keywords() {
        assert_eq!(
            classify_prompt("fix the layout bug in this screenshot"),
            AutoCapability::Vision
        );
    }

    #[test]
    fn resolve_returns_ezr_ids_from_full_pools() {
        let (cap, api, key) =
            resolve_auto_model_tuple("refactor and implement the retry helper", None);
        assert_eq!(cap, AutoCapability::CodingAgent);
        assert!(api.starts_with("ezr/"), "api id must be ezr/... got {api}");
        assert!(key.starts_with("ezr-"), "catalog key must be ezr-... got {key}");
        assert_eq!(api, "ezr/claude-sonnet-5");
        assert_eq!(key, "ezr-claude-sonnet-5");
    }

    #[test]
    fn forced_capability_overrides_classifier() {
        let r = resolve_auto_model("fix a bug", Some(AutoCapability::CheapBulk));
        assert_eq!(r.capability, AutoCapability::CheapBulk);
        assert_eq!(r.model_api_id, "ezr/gpt-5.4-nano");
    }

    #[test]
    fn in_pool_then_adjacent_failover() {
        let primary = resolve_auto_model("hello", Some(AutoCapability::ChatDefault));
        assert_eq!(primary.chain_index, 0);
        assert_eq!(primary.model_api_id, "ezr/claude-sonnet-5");

        let mut cur = primary;
        let mut seen = vec![cur.model_api_id.clone()];
        while let Some(next) = next_fallback(&cur) {
            seen.push(next.model_api_id.clone());
            cur = next;
        }
        // Home pool (6) then adjacent cheap_bulk models not already present.
        assert!(seen.len() > 6, "should spill into adjacent pool: {seen:?}");
        assert!(seen.contains(&"ezr/gpt-5.4-nano".to_string()));
        assert!(seen.iter().all(|id| id.starts_with("ezr/")));
    }

    #[test]
    fn every_capability_primary_is_ezr_from_catalog() {
        for cap in AutoCapability::all() {
            let r = resolve_auto_model("", Some(*cap));
            assert!(
                r.model_api_id.starts_with("ezr/"),
                "{cap}: {}",
                r.model_api_id
            );
            assert_eq!(r.catalog_key, catalog_key_for_api_id(&r.model_api_id));
            assert!(!capability_pool(*cap).is_empty());
        }
    }

    #[test]
    fn pool_ids_are_subset_of_known_74_names() {
        // Bare names from 9route-ezr-models.md (without ezr/ prefix).
        const KNOWN: &[&str] = &[
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-opus-4-5",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5@20250929",
            "claude-haiku-4-5",
            "claude-haiku-4-5@20251001",
            "claude-fable-5-1",
            "claude-fable-5",
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.4-nano",
            "gpt-5.2",
            "gpt-5.1",
            "gpt-chat-latest",
            "gpt-4.1",
            "gpt-4o",
            "gpt-4o-mini",
            "gemini-3.8-flash",
            "gemini-3.7-flash",
            "gemini-3.6-flash",
            "gemini-3.5-flash",
            "gemini-3.5-flash-lite",
            "gemini-3.1-pro-preview",
            "gemini-3.1-pro-preview-customtools",
            "gemini-3.1-flash-lite",
            "gemini-3-flash-preview",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
            "grok-4.6",
            "grok-4.5",
            "grok-4.3",
            "glm-5.3",
            "glm-5.3-flash",
            "glm-5.2",
            "glm-5.1",
            "glm-5",
            "glm-5-turbo",
            "glm-5v-turbo",
            "qwen3.8-max",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.6-plus",
            "qwen3-max",
            "deepseek-v4-pro",
            "deepseek-v4.1-flash",
            "deepseek-v4-flash",
            "deepseek-v4-flash-vision-exp",
            "deepseek-flash",
            "kimi-k3",
            "kimi-k2.7-code",
            "kimi-k2.7-code-highspeed",
            "kimi-k2.6",
            "MiniMax-M3",
            "MiniMax-M2.7",
            "MiniMax-M2.5",
            "MiniMax-M2.1",
            "doubao-seed-2-0-pro-260215",
            "dola-seed-2-1-turbo",
            "seed-2-0-lite-260228",
            "e-g-5.5",
            "e-g-5.6-s",
            "e-g-5.6-t",
            "e-ge-3.1-p-p",
            "e-ge-3.7-f",
        ];
        assert_eq!(KNOWN.len(), 74);
        for id in all_pool_model_ids() {
            let bare = id.strip_prefix("ezr/").unwrap_or(id);
            assert!(
                KNOWN.contains(&bare),
                "pool id {id} not in the 74-model catalog"
            );
        }
    }

    #[test]
    fn capabilities_listing_mentions_all_pools() {
        let listing = format_capabilities_listing();
        for cap in AutoCapability::all() {
            assert!(
                listing.contains(cap.as_str()),
                "missing {} in listing",
                cap.as_str()
            );
        }
        assert!(listing.contains("ezr/claude-sonnet-5"));
        assert!(listing.contains("ezr/glm-5v-turbo"));
    }

    #[test]
    fn quota_error_detector() {
        assert!(is_quota_or_rate_limit_error(
            "Error: insufficient_quota for this key"
        ));
        assert!(is_quota_or_rate_limit_error("HTTP 429 Too Many Requests"));
        assert!(!is_quota_or_rate_limit_error("context length exceeded"));
    }

    #[test]
    fn parse_capability_aliases() {
        assert_eq!(
            AutoCapability::parse("coding"),
            Some(AutoCapability::CodingAgent)
        );
        assert_eq!(
            AutoCapability::parse("heavy"),
            Some(AutoCapability::ReasoningHeavy)
        );
        assert_eq!(
            AutoCapability::parse("cheap"),
            Some(AutoCapability::CheapBulk)
        );
    }
}
