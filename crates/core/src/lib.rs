//! onyx-core: the protocol layer that decouples the agent engine from its
//! clients.
//!
//! Design (codex-level architecture, clean-room): a **Thread** is one durable
//! conversation; a **Turn** is one user request and all the work it spawns; an
//! **Item** is the smallest renderable unit of input/output with an explicit
//! lifecycle (`Started → Updated → Completed | Failed`). Clients — the TUI, an
//! app-server over JSON-RPC, a web or IDE front-end — subscribe to the event
//! stream and choose their own rendering granularity. The engine never knows
//! which consumer is watching.
//!
//! This crate carries only protocol + session mechanics (pure std + serde_json,
//! matching the workspace's no-derive serialization style). It deliberately has
//! NO dependency on `agent`/`tools`: `CoreEvent` is a lossless mirror of the
//! engine's `AgentEvent`, mapped at the boundary by whichever host owns the
//! loop. That keeps the wire format stable while the engine evolves.

pub mod jsonrpc;
pub mod protocol;
pub mod session;

pub use protocol::{
    ApprovalOutcome, CoreEvent, ItemKind, ItemStatus, TurnId, UsageFigures, EVENT_SCHEMA_VERSION,
};
pub use session::{SessionMeta, ThreadStore};
