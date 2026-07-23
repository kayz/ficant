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
    ComparisonDimension, ExecutionExternalInput, ExecutionIdentity, ExecutionIdentityInput,
    ExecutionInstanceIdentity, ExperimentComparison, NativeExecutionResult, NativeNode,
    NativeNodeArtifact, NativeNodeExecution, NativeNodeRequest, NativePortValue,
    NodeImplementation, ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding,
    canonical_output_bytes, compare_experiments, decode_canonical_output_bytes,
    execute_native_graph, execute_native_graph_with_external_inputs, execute_native_node,
    verify_native_replay,
};
pub use replay::{ReplayResult, replay};
