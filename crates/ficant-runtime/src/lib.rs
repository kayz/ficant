//! Pure append-only journal and deterministic research graph replay runtime.

mod digest;
mod graph_execution;
mod journal;
mod native_execution;
mod replay;

pub use graph_execution::{
    GraphCheckpoint, GraphNodeEvent, GraphReplayResult, replay_graph_execution,
};
pub use journal::{
    AppendResult, IdempotencyKey, JournalAppend, PerRunJournal, RuntimeError, SharedRunJournal,
};
pub use native_execution::{
    ComparisonDimension, ExecutionIdentity, ExecutionIdentityInput, ExperimentComparison,
    NativeExecutionResult, NativeNode, NativeNodeArtifact, NativeNodeRequest, NativePortValue,
    NodeImplementation, compare_experiments, execute_native_graph, verify_native_replay,
};
pub use replay::{ReplayResult, replay};
