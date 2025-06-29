#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/op-rs/kona/main/assets/square.png",
    html_favicon_url = "https://raw.githubusercontent.com/op-rs/kona/main/assets/favicon.ico",
    issue_tracker_base_url = "https://github.com/op-rs/kona/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[macro_use]
extern crate tracing;

mod service;
pub use service::{
    NodeMode, RollupNode, RollupNodeBuilder, RollupNodeError, RollupNodeService,
    SequencerNodeService, ValidatorNodeService,
};

mod actors;
pub use actors::{
    DerivationActor, DerivationError, EngineActor, EngineError, EngineLauncher,
    InboundDerivationMessage, InboundEngineMessage, L1WatcherRpc, L1WatcherRpcError, L2Finalizer,
    NetworkActor, NetworkActorError, NodeActor, RpcActor, RpcActorError, RuntimeActor,
    RuntimeLauncher, SupervisorActor, SupervisorActorError, SupervisorExt, SupervisorRpcServerExt,
};

mod metrics;
pub use metrics::Metrics;
