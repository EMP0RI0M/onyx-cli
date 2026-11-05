//! Per-turn capability selection — the retrieval engine wired into the loop.
//!
//! The problem this solves: onyx ships *every* tool definition to the model on
//! *every* step. That is fine at a dozen tools and actively harmful at a
//! hundred — the catalog eats the context budget, and a model offered forty
//! plausible tools picks worse than one offered six. [`CapabilitySelector`]
//! keeps the full corpus outside the prompt and retrieves only what the current
//! turn's intent points at.
//!
//! Two decisions worth stating explicitly, because they are load-bearing:
//!
//! - **BM25 only, by default.** Dense and hybrid retrieval need an embedding
//!   model, which means a download and a candle inference pass per corpus. Lexical
//!   BM25 needs neither, cannot fail, and is deterministic — so it is what runs
//!   unless `ONYX_RETRIEVAL=hybrid` is set *and* embeddings built. A host that
//!   cannot retrieve must still be able to run the agent.
//! - **Retrieval never strands the agent.** The always-present core (terminate,
//!   ask_human) is injected regardless of score, and a search that returns nothing
//!   falls back to the full catalog rather than sending a tool-less request. When
//!   the corpus is small enough that filtering would hide more than it saves, the
//!   selector says so and offers everything.
//!
//! Adaptive ranking (the usage arm) is opt-in via `ONYX_USAGE_RANKING=1`: the
//! registry's trace sink becomes a [`UsageLearner`] that writes an [`IntentGraph`]
//! from real searches and invokes, and the same graph is attached to both catalogs
//! as the read model. Ranking then learns which capabilities a given kind of
//! request actually gets used.

use std::sync::{Arc, RwLock};

use serde_json::{json, Value};

use onyx_retrieval::{
    IntentGraph, NoopSink, Origin, SearchMethod, SearchOptions, SkillRegistry, ToolRegistry,
    TraceEvent, TraceSink, UsageLearner,
};

use crate::model::HttpModel;
use crate::Message;

/// Tools the model must be able to reach on every step no matter what the
/// retriever scored. `terminate` ends the turn; without it a filtered request can
/// only be answered by running out of steps.
const CORE_TOOLS: &[&str] = &["terminate"];

/// How many capabilities a turn may see. Sixteen definitions is roughly 1-2k
/// tokens of catalog — enough for a real choice, small enough that the catalog is
/// not the majority of the prompt.
const DEFAULT_TOP_K: usize = 16;

/// Below this many registered tools, ranking changes nothing worth the risk of
/// hiding the one tool the task needed: offer the whole catalog.
const MIN_CATALOG_FOR_FILTERING: usize = 8;

/// How many skills to unfold into the prompt when the turn's intent matches one.
/// One skill body is already a page of instructions; two unrelated playbooks in
/// one prompt is how agents start following the wrong recipe.
const DEFAULT_SKILL_TOP_K: usize = 1;

fn env_number(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).filter(|n: &usize| *n > 0).unwrap_or(default)
}

/// Tools the model must always be able to reach. Computed once per process from
/// the environment; see [`env_number`].
fn core_tools() -> &'static [String] {
    static CORE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CORE.get_or_init(|| {
        let mut v: Vec<String> = CORE_TOOLS.iter().map(|s| s.to_string()).collect();
        for extra in std::env::var("ONYX_CORE_TOOLS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            if !v.iter().any(|existing| existing == extra) {
                v.push(extra.to_string());
            }
        }
        v
    })
}

/// What one retrieval decided, kept for the `/tools` panel and the audit trail.
#[derive(Debug, Clone)]
pub struct TurnSelection {
    /// Capabilities offered to the model this turn, best first.
    pub offered: Vec<String>,
    /// Skills whose bodies were unfolded into the system prompt.
    pub skills: Vec<String>,
    /// Registry size vs. how many were offered — "8 of 40", i.e. the compression
    /// the model actually got. Equal numbers mean the catalog was passed through.
    pub catalog_size: usize,
    /// True when the corpus was too small to filter, or the search found nothing.
    pub passthrough: bool,
    /// `"bm25"` or `"hybrid"` — which engine produced this.
    pub method: &'static str,
}

impl TurnSelection {
    /// Human summary line: `tools 8/40 via bm25`.
    pub fn summary(&self) -> String {
        let mode = if self.passthrough { "all" } else { &self.method };
        format!(
            "offered {} of {} tools via {mode}; skills: {}",
            self.offered.len(),
            self.catalog_size,
            if self.skills.is_empty() { "none".to_string() } else { self.skills.join(", ") }
        )
    }
}

/// The retrieval-backed view of onyx's capability catalog.
///
/// Owns the two registries plus the shared usage graph, and knows how to project
/// a ranked subset back onto the wire format the providers expect. Cloning is
/// cheap-ish (registry contents are cloned), so hand one to a worker thread
/// rather than fighting the borrow checker over a shared reference.
pub struct CapabilitySelector {
    tools: ToolRegistry,
    skills: SkillRegistry,
    /// Read model for adaptive ranking, shared by both catalogs. `None` when the
    /// usage arm is off — then neither registry has a learner and scores are pure
    /// content relevance.
    graph: Option<Arc<RwLock<IntentGraph>>>,
    top_k: usize,
    skill_top_k: usize,
    min_catalog: usize,
    hybrid: bool,
}

impl CapabilitySelector {
    /// An empty selector with defaults from the environment. Build registries into
    /// it with [`Self::with_tools`] / [`Self::with_skills`].
    pub fn new() -> Self {
        let hybrid = std::env::var("ONYX_RETRIEVAL")
            .map(|v| v.eq_ignore_ascii_case("hybrid"))
            .unwrap_or(false);
        Self {
            tools: ToolRegistry::new(),
            skills: SkillRegistry::new(),
            graph: None,
            top_k: env_number("ONYX_TOOL_TOP_K", DEFAULT_TOP_K),
            skill_top_k: env_number("ONYX_SKILL_TOP_K", DEFAULT_SKILL_TOP_K),
            min_catalog: env_number("ONYX_MIN_FILTER_CATALOG", MIN_CATALOG_FOR_FILTERING),
            hybrid,
        }
    }

    /// Enable adaptive ranking: wrap each registry's sink in a [`UsageLearner`]
    /// writing a shared [`IntentGraph`], and attach that graph as both registries'
    /// read model. Idempotent. Without embeddings the usage arm still boosts — the
    /// graph is lexical in that configuration, so this works on a BM25-only host.
    pub fn enable_usage_ranking(&mut self) -> &mut Self {
        if self.graph.is_some() {
            return self;
        }
        let graph = Arc::new(RwLock::new(IntentGraph::empty()));
        let tool_learner = Arc::new(UsageLearner::new(graph.clone(), Arc::new(NoopSink)));
        self.tools.set_trace_sink(tool_learner);
        self.tools.set_intent_graph(Some(graph.clone()));
        let skill_learner = Arc::new(UsageLearner::new(graph.clone(), Arc::new(NoopSink)));
        self.skills.set_trace_sink(skill_learner);
        self.skills.set_intent_graph(Some(graph.clone()));
        self.graph = Some(graph);
        self
    }

    /// Whether adaptive ranking is wired up (the `/tools` panel reports this).
    pub fn usage_ranking_enabled(&self) -> bool {
        self.graph.is_some()
    }

    /// Replace the tool corpus wholesale. Registration invalidates the BM25 index,
        /// so re-registering is the supported way to pick up MCP tools discovered
    /// mid-session.
    pub fn with_tools(mut self, tools: Vec<onyx_retrieval::Tool>) -> Self {
        for t in tools {
            self.tools.register(t);
        }
        self
    }

    /// Replace the skill corpus wholesale.
    pub fn with_skills(mut self, skills: Vec<onyx_retrieval::Skill>) -> Self {
        self.skills.replace_all(skills);
        self
    }

    /// Registered tool count.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Registered skill count.
    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }

    /// Override how many capabilities one turn may see.
    pub fn with_top_k(mut self, k: usize) -> Self {
        if k > 0 {
            self.top_k = k;
        }
        self
    }

    /// Override the corpus size below which filtering is skipped entirely.
    pub fn with_min_catalog(mut self, n: usize) -> Self {
        self.min_catalog = n;
        self
    }

    /// Ask for semantic/hybrid ranking. Honesty note: this only takes effect once
    /// [`Self::build_embeddings`] has covered both corpora; until then every search
    /// stays BM25 and [`TurnSelection::method`] says `"bm25"`.
    pub fn with_hybrid(mut self, hybrid: bool) -> Self {
        self.hybrid = hybrid;
        self
    }

    /// The engine this selector will use for the next turn. Reported honestly:
    /// asking for hybrid without built embeddings degrades to BM25, and the caller
    /// needs to see that rather than be told a lie by its own config.
    fn effective_method(&self) -> SearchMethod {
        if self.hybrid && self.embeddings_ready() {
            SearchMethod::Hybrid
        } else {
            SearchMethod::Bm25
        }
    }

    /// Whether both catalogs have dense vectors built and still cover their corpus
    /// (see [`ToolRegistry::build_embeddings`]). Registering a tool invalidates it,
    /// so this is false again after any catalog churn until the next build.
    pub fn embeddings_ready(&self) -> bool {
        self.tools.embeddings_ready() && self.skills.embeddings_ready()
    }

    /// Build the dense embedding cache for both catalogs. Loads the model on first
    /// call, which means a download the first time on a machine with no HF cache —
    /// so this is never implicit, only from `ONYX_EMBED=1` at startup or an explicit
    /// `/tools embed`.
    ///
    /// # Errors
    /// Whatever the embedder reports: no model cached and no network, a failed
    /// endpoint call, or a dimension mismatch.
    pub fn build_embeddings(&mut self) -> Result<(), String> {
        self.tools
            .build_embeddings()
            .map_err(|e| format!("tool embeddings: {e}"))?;
        self.skills
            .build_embeddings()
            .map_err(|e| format!("skill embeddings: {e}"))?;
        Ok(())
    }

    /// Rank the corpus against one intent and write the result onto the model.
    ///
    /// This is the whole integration: one search per turn, then the request surface
    /// for *that turn* is rebuilt from just the winners. Everything downstream —
    /// every provider protocol, streaming and non-streaming alike — reads through
    /// [`HttpModel::active_tools`], so setting `turn_tools` here filters all of them
    /// at once. The session's own `catalog`/`tool_defs` are left untouched, which is
    /// what lets a narrow turn widen back out on the next one.
    pub fn apply_turn<P: AsRef<str>>(&self, hm: &mut HttpModel, query: P, history: &[Message]) {
        let Some(selection) = self.select(query.as_ref(), history) else {
            return; // nothing to rank: leave the full catalog exactly as built
        };
        if selection.passthrough {
            hm.turn_tools = None;
            return;
        }
        let mut defs: Vec<Value> = Vec::with_capacity(selection.offered.len());
        let mut catalog = String::new();
        for name in &selection.offered {
            let Some(tool) = self.tools.get(name) else { continue };
            defs.push(json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                }
            }));
            catalog.push_str(&format!("  - {}: {}\n", tool.name, tool.description));
        }
        hm.turn_tools = Some(crate::model::TurnTools {
            catalog,
            defs,
            skills: self.skill_instructions(&selection.skills),
        });
    }

    /// Decide which capabilities a turn should see. Returns `None` when there is no
    /// tool corpus to rank at all (a text-only session), which the caller must treat
    /// as "change nothing" rather than "offer nothing".
    pub fn select(&self, query: &str, history: &[Message]) -> Option<TurnSelection> {
        if self.tools.is_empty() {
            return None;
        }
        let catalog_size = self.tools.len();
        let method = self.effective_method();
        let label = if matches!(method, SearchMethod::Hybrid) { "hybrid" } else { "bm25" };

        // Too small to be worth filtering, or filtering could cost the one tool the
        // task needed. Say so and keep the full catalog. Inclusive: a corpus exactly
        // at the threshold is still too small to justify the risk of hiding a tool.
        if catalog_size <= self.min_catalog {
            let offered: Vec<String> = self.tools.names_in_order();
            return Some(TurnSelection {
                offered,
                skills: self.matched_skills(query),
                catalog_size,
                passthrough: true,
                method: label,
            });
        }

        // Context matters: a follow-up ("now do the same to the other file") carries
        // almost no rankable terms on its own. Fold in the tail of the conversation so
        // the query is the turn plus what it refers to, bounded so a long transcript
        // cannot swamp the current intent.
        let enriched = enrich_query(query, history);
        let hits = match self
            .tools
            .search_with_options(&enriched, self.top_k, Origin::Agent, SearchOptions::new(method))
        {
            Ok(hits) => Some(hits),
            // Dense/hybrid failed (model unavailable): fall back to the engine that
            // cannot fail rather than dropping the turn's capabilities.
            Err(_) => self
                .tools
                .search_with_options(
                    &enriched,
                    self.top_k,
                    Origin::Agent,
                    SearchOptions::new(SearchMethod::Bm25),
                )
                .ok(),
        }
        .unwrap_or_default();

        let mut offered: Vec<String> = hits.iter().map(|h| h.tool_id.clone()).collect();
        for core in core_tools() {
            if self.tools.contains(core) && !offered.iter().any(|n| n == core) {
                offered.push(core.clone());
            }
        }
        if offered.is_empty() {
            // Nothing matched — the honest fallback is the full catalog, not a
            // request with zero tools that can only end in MaxSteps.
            return Some(TurnSelection {
                offered: self.tools.names_in_order(),
                skills: self.matched_skills(query),
                catalog_size,
                passthrough: true,
                method: label,
            });
        }
        Some(TurnSelection {
            offered,
            skills: self.matched_skills(query),
            catalog_size,
            passthrough: false,
            method: label,
        })
    }

    /// Skills whose description/tags match the intent, best-first. Empty corpus →
    /// empty list (never an error).
    fn matched_skills(&self, query: &str) -> Vec<String> {
        if self.skills.is_empty() {
            return Vec::new();
        }
        let method = self.effective_method();
        let hits = match self.skills.search_with_options(
            query,
            self.skill_top_k,
            Origin::Agent,
            SearchOptions::new(method),
        ) {
            Ok(hits) => Some(hits),
            Err(_) => self.skills.search_with_options(
                query,
                self.skill_top_k,
                Origin::Agent,
                SearchOptions::new(SearchMethod::Bm25),
            ).ok(),
        }
        .unwrap_or_default();
        hits.iter().map(|h| h.skill_id.clone()).collect()
    }

    /// The rendered instruction blocks for the skills a turn matched, in rank order.
    /// Appended to the system prompt by the caller — a skill body is instructions, so
    /// it belongs with the system message, not as a fake user turn.
    pub fn skill_instructions(&self, ids: &[String]) -> String {
        let mut out = String::new();
        for id in ids {
            let Some(skill) = self.skills.get(id) else { continue };
            out.push_str(&format!(
                "\n\n[Retrieved skill: {}]\n{}\n{}",
                skill.name, skill.description, skill.body
            ));
        }
        out
    }

    /// Record that the model actually invoked a tool, so adaptive ranking can
    /// credit the search that offered it. Emitted on the registry's sink, which is
    /// the [`UsageLearner`] when the usage arm is on and a no-op otherwise — so this
    /// is always safe to call and costs nothing when disabled.
    pub fn record_invoke(&self, tool_id: &str, args: &Value, ok: bool, took_ms: u64, error: &str) {
        let size = args.to_string().len() as u64;
        self.tools.record_event(TraceEvent::InvokeStart {
            tool_id: tool_id.to_string(),
            args_size_bytes: size,
        });
        self.tools.record_event(if ok {
            TraceEvent::InvokeEnd { tool_id: tool_id.to_string(), took_ms }
        } else {
            TraceEvent::InvokeError {
                tool_id: tool_id.to_string(),
                took_ms,
                error: error.to_string(),
            }
        });
    }

    /// Record that a retrieved skill was actually followed.
    pub fn record_skill_invoke(&self, skill_id: &str, took_ms: u64) {
        self.skills
            .record_event(TraceEvent::SkillInvoke { skill_id: skill_id.to_string(), took_ms });
    }

    /// Status of adaptive ranking as a short report for the TUI: whether the usage
    /// arm is influencing scores, and if not, why.
    pub fn adaptive_status(&self) -> String {
        if self.graph.is_none() {
            return "off (ONYX_USAGE_RANKING=1 to enable)".to_string();
        }
        self.tools.adaptive_ranking_status_name()
    }

    /// Names of every registered tool, in corpus order — what the unfiltered
    /// catalog offers.
    pub fn all_tool_names(&self) -> Vec<String> {
        self.tools.names_in_order()
    }

    /// Sink the tool registry writes trace events to, for hosts that want the
    /// stream (the inspector, a JSONL file, or a fanout to both).
    pub fn tool_sink(&self) -> Arc<dyn TraceSink> {
        self.tools.sink()
    }
}

impl Default for CapabilitySelector {
    fn default() -> Self {
        Self::new()
    }
}

/// Fold the recent conversation into the retrieval query.
///
/// Kept deliberately short: the current prompt leads (it is the intent), and only
/// the last few messages contribute, truncated per message. Without this a
/// pronoun-only follow-up retrieves almost nothing useful; with too much context
/// the query drifts toward whatever was said twenty turns ago.
fn enrich_query(query: &str, history: &[Message]) -> String {
    if history.is_empty() {
        return query.to_string();
    }
    const TAIL: usize = 3;
    const PER_MESSAGE: usize = 200;
    let mut parts: Vec<String> = Vec::with_capacity(TAIL + 1);
    parts.push(query.to_string());
    for m in history.iter().rev().take(TAIL) {
        if m.role == "system" || m.content.trim().is_empty() {
            continue;
        }
        let clipped: String = m.content.chars().take(PER_MESSAGE).collect();
        parts.push(clipped);
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, desc: &str) -> onyx_retrieval::Tool {
        onyx_retrieval::Tool {
            id: name.into(),
            name: name.into(),
            description: desc.into(),
            experimental_searchable_description: None,
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: json!({}),
        }
    }

    fn corpus(n: usize) -> Vec<onyx_retrieval::Tool> {
        let mut v = vec![
            tool("read_file", "Read a file from disk and return its contents"),
            tool("write_file", "Write or overwrite a file on disk"),
            tool("bash", "Run a shell command and capture its output"),
            tool("grep", "Search file contents for a pattern"),
            tool("web_search", "Search the web for a query string"),
            tool("vecmath", "Multiply matrices using the NEON kernel"),
            tool("python", "Evaluate a Python expression"),
            tool("terminate", "End the turn with the final answer"),
        ];
        for i in v.len()..n {
            v.push(tool(&format!("filler_{i}"), "An unrelated placeholder capability"));
        }
        v
    }

    fn selector(tools: Vec<onyx_retrieval::Tool>) -> CapabilitySelector {
        // The filtering tests need a corpus above the pass-through threshold and an
        // explicit top-k, so they do not depend on this box's environment.
        CapabilitySelector::new().with_top_k(6).with_min_catalog(8).with_tools(tools)
    }
    /// A model whose session catalog holds all `n` corpus tools. Built from a fresh
    /// corpus because [`onyx_retrieval::Tool`] is not `Clone` — the selector and the
    /// model each register their own copy of the same definitions.
    fn model_with_full_catalog(n: usize) -> HttpModel {
        let tools = corpus(n);
        let mut hm = HttpModel::new(
            "127.0.0.1".into(),
            1,
            "test-model".into(),
            String::new(),
            String::new(),
        );
        hm.catalog = tools
            .iter()
            .map(|t| format!("  - {}: {}\n", t.name, t.description))
            .collect::<String>();
        hm.tool_defs = tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.input_schema
                }})
            })
            .collect();
        hm
    }

    /// The tool definitions this turn will actually send — the projection if there
    /// is one, otherwise the session catalog.
    fn offered_defs(hm: &HttpModel) -> &[Value] {
        match &hm.turn_tools {
            Some(t) => &t.defs,
            None => &hm.tool_defs,
        }
    }

    fn offered_names(hm: &HttpModel) -> Vec<&str> {
        offered_defs(hm)
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn small_catalog_is_passed_through_not_filtered() {
        // Eight tools is below the threshold where filtering can pay for its risk.
        let sel = selector(corpus(8));
        let mut hm = model_with_full_catalog(8);
        let before = hm.tool_defs.len();
        sel.apply_turn(&mut hm, "read a file", &[]);
        assert_eq!(offered_defs(&hm).len(), before, "small catalog must stay intact");
        assert!(hm.turn_tools.is_none(), "pass-through must not install a projection");
    }

    #[test]
    fn large_catalog_offers_only_matching_tools() {
        let sel = selector(corpus(40));
        let mut hm = model_with_full_catalog(40);
        sel.apply_turn(&mut hm, "read the contents of a file from disk", &[]);
        let offered = offered_defs(&hm);
        assert!(offered.len() <= sel.top_k, "offered {} > top_k", offered.len());
        assert!(offered.len() < 40, "nothing was filtered");
        let names = offered_names(&hm);
        assert!(names.contains(&"read_file"), "the matching tool was missed: {names:?}");
        assert!(!names.contains(&"web_search"), "an irrelevant tool leaked in: {names:?}");
        // Session catalog untouched: narrowing one turn must not narrow the session.
        assert_eq!(hm.tool_defs.len(), 40, "apply_turn mutated the session");
    }

    #[test]
    fn terminate_survives_filtering() {
        // Without this the filtered turn can only end by exhausting max_steps.
        let sel = selector(corpus(40));
        let mut hm = model_with_full_catalog(40);
        sel.apply_turn(&mut hm, "multiply two matrices", &[]);
        let names = offered_names(&hm);
        assert!(names.contains(&"terminate"), "terminate missing from {names:?}");
    }

    #[test]
    fn catalog_text_and_defs_stay_in_agreement() {
        // The prose catalog and the function definitions describe one set; a model
        // told about a tool it cannot call will call it and fail.
        let sel = selector(corpus(40));
        let mut hm = model_with_full_catalog(40);
        sel.apply_turn(&mut hm, "run a shell command", &[]);
        let catalog = &hm.turn_tools.as_ref().expect("expected a projection").catalog;
        for def in offered_defs(&hm) {
            let name = def["function"]["name"].as_str().unwrap();
            assert!(catalog.contains(&format!("  - {name}:")), "{name} is callable but not described");
        }
    }

    #[test]
    fn empty_corpus_leaves_the_model_alone() {
        // A text-only session has nothing to rank; applying a turn must not blank
        // out whatever catalog the host built.
        let sel = selector(Vec::new());
        let mut hm = HttpModel::new("h".into(), 1, "m".into(), String::new(), "  - x: y\n".into());
        hm.tool_defs = vec![json!({"type": "function"})];
        sel.apply_turn(&mut hm, "anything", &[]);
        assert_eq!(hm.tool_defs.len(), 1);
        assert_eq!(hm.catalog, "  - x: y\n");
        assert!(hm.turn_tools.is_none());
    }

    #[test]
    fn followup_query_retrieves_from_conversation_context() {
        // "now do it to the other one" has no rankable terms; the folded history does.
        let sel = selector(corpus(40));
        let mut hm = model_with_full_catalog(40);
        let history = vec![
            Message::text("user", "read the contents of the config file from disk"),
            Message::text("assistant", "here it is"),
        ];
        sel.apply_turn(&mut hm, "now do it for the other one", &history);
        let names = offered_names(&hm);
        assert!(names.contains(&"read_file"), "context fold did not help: {names:?}");
    }

    #[test]
    fn retrieved_skills_are_rendered_into_the_turn() {
        // A matched skill's body must reach the prompt through the projection, not
        // by the caller remembering to append it.
        let mut sel = CapabilitySelector::new().with_top_k(6).with_min_catalog(8).with_tools(corpus(40));
        sel = sel.with_skills(vec![onyx_retrieval::Skill {
            id: "pdf".into(),
            name: "pdf".into(),
            description: "Extract text and tables from PDF documents".into(),
            experimental_searchable_description: None,
            tags: vec!["documents".into(), "pdf".into()],
            tools: vec![],
            metadata: Default::default(),
            body: "Step 1: open the file with pdfplumber.".into(),
        }]);
        let mut hm = model_with_full_catalog(40);
        sel.apply_turn(&mut hm, "pull the tables out of this pdf document", &[]);
        let skills = &hm.turn_tools.as_ref().expect("expected a projection").skills;
        assert!(skills.contains("[Retrieved skill: pdf]"), "skill not rendered: {skills:?}");
        assert!(skills.contains("pdfplumber"));
    }

    #[test]
    fn skills_are_matched_by_intent_and_rendered() {
        let mut sel = CapabilitySelector::new().with_tools(corpus(40));
        sel = sel.with_skills(vec![
            onyx_retrieval::Skill {
                id: "pdf".into(),
                name: "pdf".into(),
                description: "Extract text and tables from PDF documents".into(),
                experimental_searchable_description: None,
                tags: vec!["documents".into(), "pdf".into()],
                tools: vec![],
                metadata: Default::default(),
                body: "Step 1: open the file with pdfplumber.".into(),
            },
            onyx_retrieval::Skill {
                id: "sql".into(),
                name: "sql".into(),
                description: "Write and optimise SQL queries".into(),
                experimental_searchable_description: None,
                tags: vec!["database".into()],
                tools: vec![],
                metadata: Default::default(),
                body: "Always check the index.".into(),
            },
        ]);
        let sel_out = sel.select("pull the tables out of this pdf", &[]).unwrap();
        assert_eq!(sel_out.skills, vec!["pdf".to_string()], "wrong skill unfolded");
        let text = sel.skill_instructions(&sel_out.skills);
        assert!(text.contains("[Retrieved skill: pdf]") && text.contains("pdfplumber"));
        assert!(!text.contains("Always check the index"), "unmatched skill leaked its body");
    }

    #[test]
    fn usage_ranking_records_invokes_without_breaking_search() {
        // The learner wraps the sink; searches and invokes must still work, and the
        // graph must actually learn.
        let mut sel = CapabilitySelector::new().with_tools(corpus(40));
        sel.enable_usage_ranking();
        assert!(sel.usage_ranking_enabled());
        let out = sel.select("read a file from disk", &[]).unwrap();
        assert!(!out.offered.is_empty());
        sel.record_invoke("read_file", &json!({"path": "/x"}), true, 5, "");
        let graph = sel.graph.as_ref().unwrap().read().unwrap();
        assert_eq!(graph.len(), 1, "invoke did not train the usage graph");
        assert_eq!(sel.adaptive_status(), "active");
    }

    #[test]
    fn adaptive_status_reports_off_when_not_wired() {
        let sel = selector(corpus(12));
        assert!(sel.adaptive_status().starts_with("off"), "{}", sel.adaptive_status());
    }

    #[test]
    fn double_enable_does_not_duplicate_learners() {
        let mut sel = CapabilitySelector::new().with_tools(corpus(12));
        sel.enable_usage_ranking();
        let first = sel.adaptive_status();
        sel.enable_usage_ranking();
        assert_eq!(first, sel.adaptive_status(), "re-enable changed the wiring");
    }

    #[test]
    fn selection_summary_reports_compression() {
        let sel = selector(corpus(40));
        let s = sel.select("read a file from disk", &[]).unwrap();
        let line = s.summary();
        assert!(line.contains("of 40 tools"), "unexpected summary: {line}");
        assert!(!s.passthrough);
    }

    #[test]
    fn env_overrides_take_effect() {
        // Top-K is operator-tunable; verify the knob is read, not hardcoded away.
        // Built from `new()` directly because the shared helper pins top_k.
        std::env::set_var("ONYX_TOOL_TOP_K", "2");
        let sel = CapabilitySelector::new().with_tools(corpus(40));
        assert_eq!(sel.top_k, 2);
        std::env::remove_var("ONYX_TOOL_TOP_K");
    }

    #[test]
    fn enrich_query_bounds_history() {
        let history: Vec<Message> = (0..20).map(|_| Message::text("user", "z".repeat(500))).collect();
        let q = enrich_query("go", &history);
        // Current query plus at most TAIL messages, each clipped.
        assert!(q.starts_with("go"));
        assert!(q.chars().count() <= 3 * 200 + 3 + 2, "query grew unbounded: {}", q.chars().count());
    }
}
