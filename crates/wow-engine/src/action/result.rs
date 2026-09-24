use std::collections::BTreeSet;
use wow_domain::{ActionFailure, ActionId};

#[derive(Clone, Debug)]
pub enum ExecutionResult {
    Submitted(ActionId),
    Completed(ActionId),
    Failed(ActionId, ActionFailure),
}

#[derive(Default)]
pub struct TerminalResults {
    terminal: BTreeSet<ActionId>,
}

impl TerminalResults {
    pub fn record(&mut self, result: &ExecutionResult) -> bool {
        match result {
            ExecutionResult::Submitted(_) => true,
            ExecutionResult::Completed(id) | ExecutionResult::Failed(id, _) => {
                self.terminal.insert(*id)
            }
        }
    }
    pub fn is_terminal(&self, id: ActionId) -> bool {
        self.terminal.contains(&id)
    }
}
