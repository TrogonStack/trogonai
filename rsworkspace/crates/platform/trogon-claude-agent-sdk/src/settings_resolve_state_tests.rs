use std::{io, path::PathBuf};

use super::ResolverState;
use crate::adapters::{CurrentDirectory, capture_launch_directory};

struct MovedAway;

impl CurrentDirectory for MovedAway {
    type Error = io::Error;

    fn current_dir(&self) -> Result<PathBuf, Self::Error> {
        Ok(PathBuf::from("/moved/away"))
    }
}

#[test]
fn the_process_state_keeps_the_captured_launch_directory() {
    let state = ResolverState::for_process();

    assert_eq!(state.launch_directory(&MovedAway), capture_launch_directory());
}
