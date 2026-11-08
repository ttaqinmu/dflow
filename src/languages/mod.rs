pub mod python;
pub mod rust;
use crate::analysis::{DiscoveredUnit, Discovery, ParsedFile};
use crate::git::{CapturedFile, CapturedSnapshot, SnapshotRole};

pub trait LanguageAdapter {
    fn accepts_source(&self, path: &[u8]) -> bool;
    fn accepts_metadata(&self, _path: &[u8]) -> bool {
        false
    }
    fn discover(&self, snapshot: &CapturedSnapshot) -> Discovery;
    fn extract(&self, file: &CapturedFile, role: SnapshotRole, unit: &DiscoveredUnit)
    -> ParsedFile;
}
pub(crate) fn adapters() -> Vec<Box<dyn LanguageAdapter>> {
    vec![Box::new(python::PythonAdapter), Box::new(rust::RustAdapter)]
}
/// Source and static metadata paths needed by registered language adapters.
pub fn capture_path(path: &[u8]) -> bool {
    adapters()
        .iter()
        .any(|adapter| adapter.accepts_source(path) || adapter.accepts_metadata(path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    DynamicTarget,
    Shadowed,
    AmbiguousBinding,
    AmbiguousTarget,
    MissingTarget,
    ExternalTarget,
    WildcardImport,
    UnknownModule,
    UnsupportedDispatch,
    DecoratedTarget,
    ParseError,
    ConditionalCompilation,
    MacroExpansion,
}
#[derive(Debug, Clone)]
pub enum Resolution<'a> {
    Resolved(&'a crate::analysis::Symbol),
    Unresolved(UnresolvedReason),
}
pub struct ResolverRegistry<'a> {
    python: python::resolve::PythonIndex<'a>,
    rust: rust::resolve::RustIndex<'a>,
}
impl<'a> ResolverRegistry<'a> {
    pub fn new(snapshot: &'a crate::analysis::ExtractedSnapshot) -> Self {
        Self {
            python: python::resolve::PythonIndex::new(snapshot),
            rust: rust::resolve::RustIndex::new(snapshot),
        }
    }
    pub fn resolve(
        &self,
        file: &'a crate::analysis::ParsedFile,
        call: &crate::analysis::ExtractedCallSite,
    ) -> Resolution<'a> {
        match file.language {
            crate::analysis::LanguageId::Python => self.python.resolve(file, call),
            crate::analysis::LanguageId::Rust => self.rust.resolve(file, call),
        }
    }
}
