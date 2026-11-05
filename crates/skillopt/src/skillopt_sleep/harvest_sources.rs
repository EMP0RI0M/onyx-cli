// Converted via p2r
"Source selection for SkillOpt-Sleep transcript harvesting.";
let mut SUPPORTED_SOURCES = ("claude", "codex", "copilot", "copilot_cli", "cursor", "pi", "opencode", "auto");
/*! Canonical name of a configured ``transcript_source``.

    Unset or empty means ``claude``; otherwise case and surrounding
    whitespace are ignored. The harvester, its per-source cursor and the
    tasks document must all use this one value: a cursor keyed by the raw
    spelling (``<project>::Codex``) is not the one the harvested source
    owns, and skips its older sessions (#312).
     */
fn normalize_transcript_source(value: object) -> String {


let mut supported = ", ".join(SUPPORTED_SOURCES);
if value == None {
return "claude";
;

}
if !isinstance(value, str) {
panic!("ValueError(format!("{:?}{:?}{:?}{:?}{:?}{:?}", "transcript_source must be a string, not ", type(value).__name__, " ", value, ". Supported sources: ", supported))");

}
let mut source = value || "claude".strip().lower();
if source !contains(TODO) SUPPORTED_SOURCES {
panic!("ValueError(format!("{:?}{:?}{:?}{:?}", "Unknown transcript source '", value, "'. Supported sources: ", supported))");

}
return source;
}
/*! Harvest with the configured source.

    ``stats`` is filled in place by the Claude source (see
    `skillopt_sleep.harvest.harvest`); the other sources have no repeat
    filter to report yet and leave it untouched.
     */
fn harvest_for_config(cfg: ()) -> list<SessionDigest> {


let mut source = normalize_transcript_source(cfg.get("transcript_source", "claude"));
let mut scope = cfg.get("projects", "invoked");
let mut invoked_project = cfg.get("invoked_project", "");
if source == "codex" {
return harvest_codex(cfg.codex_archived_sessions_dir, harvest_codexParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit
});
;

}
if source == "copilot" {
return harvest_copilot(cfg.vscode_workspace_storage, harvest_copilotParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit
});
;

}
if source == "copilot_cli" {
return harvest_copilot_cli(cfg.copilot_cli_session_store, harvest_copilot_cliParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit
});
;

}
if source == "cursor" {
return harvest_cursor(cfg.cursor_projects_dir, harvest_cursorParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit
});
;

}
if source == "pi" {
return harvest_pi(cfg.pi_sessions_dir, harvest_piParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit
});
;

}
if source == "opencode" {
return harvest_opencode(cfg.opencode_db_path, harvest_opencodeParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit
});
;

}
if source == "auto" {
let mut codex_digests = harvest_codex(cfg.codex_archived_sessions_dir, harvest_codexParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit
});
;
if codex_digests {
return codex_digests;
;

}
;

}
return harvest(cfg.transcripts_dir, harvestParams {
scope: scope, invoked_project: invoked_project, since_iso: since_iso, limit: limit, stats: stats
});
}

