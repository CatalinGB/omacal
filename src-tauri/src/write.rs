//! Re-export of the moved module, kept so every `crate::write::…` call site
//! in `src-tauri` is unchanged. The implementation lives in `omacal-view`, the
//! Tauri-free crate the server will also depend on.

pub use omacal_view::write::*;
