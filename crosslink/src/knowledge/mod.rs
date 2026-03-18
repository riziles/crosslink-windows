mod core;
mod edit;
mod pages;
mod search;
mod sync;

#[cfg(test)]
mod tests;

// Re-export public API so `crate::knowledge::*` paths remain unchanged.
#[allow(unused_imports)]
pub use self::core::{
    has_conflict_markers, parse_frontmatter, resolve_accept_both, serialize_frontmatter,
    KnowledgeManager, PageFrontmatter, PageInfo, SearchMatch, Source, SyncOutcome,
    KNOWLEDGE_BRANCH, KNOWLEDGE_CACHE_DIR,
};

// Re-export edit helpers used by commands/knowledge/operations.rs
#[allow(unused_imports)]
pub use self::edit::{append_to_section_content, extract_body, replace_section_content, truncate};
