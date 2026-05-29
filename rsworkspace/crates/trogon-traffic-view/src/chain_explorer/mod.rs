mod errors;
mod render;
mod tree;

pub use errors::ChainExplorerError;
pub use render::{RenderOpts, render_tree};
pub use tree::{ChainNode, ChainTree};

/// Builds delegation trees from indexed traffic rows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChainExplorer;

impl ChainExplorer {
    pub fn build_tree(
        root_request_id: &str,
        events: Vec<crate::event::TrafficEvent>,
    ) -> Result<ChainTree, ChainExplorerError> {
        ChainTree::from_events(root_request_id, events)
    }
}
