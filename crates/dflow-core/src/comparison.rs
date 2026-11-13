#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalSelection {
    Combined,
    Staged,
    Unstaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparison {
    Local(LocalSelection),
    Branch { base: String, target: String },
}
