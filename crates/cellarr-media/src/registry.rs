//! The `MediaType` → module registry.
//!
//! The pipeline holds a registry and asks it for the module matching a content
//! node's [`MediaType`]; it never names a concrete module type. Because
//! [`MediaModule`] carries an associated `Error`, a heterogeneous collection of
//! modules cannot be stored behind a single `dyn MediaModule`. The registry
//! therefore stores modules behind [`DynMediaModule`] — an object-safe facade
//! with a uniform boxed error — and a blanket impl adapts any [`MediaModule`] to
//! it. This is the one place the error types are unified, so individual modules
//! keep their precise typed errors.

use std::collections::HashMap;

use async_trait::async_trait;

use cellarr_core::{
    ContentMatch, ContentRef, MediaModule, MediaType, NamingTokens, ParsedRelease, SearchTerms,
};

/// A boxed, type-erased module error (the registry's uniform error).
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Object-safe facade over [`MediaModule`] with a uniform boxed error.
///
/// Identical surface to [`MediaModule`], minus the associated `Error` (boxed
/// instead), so a registry can hold modules of different concrete error types.
#[async_trait]
pub trait DynMediaModule: Send + Sync {
    /// The media type this module serves.
    fn media_type(&self) -> MediaType;
    /// See [`MediaModule::search_terms`].
    async fn search_terms(&self, content: &ContentRef) -> Result<SearchTerms, BoxError>;
    /// See [`MediaModule::match_release`].
    async fn match_release(&self, parsed: &ParsedRelease) -> Result<Vec<ContentMatch>, BoxError>;
    /// See [`MediaModule::naming_tokens`].
    async fn naming_tokens(&self, content: &ContentRef) -> Result<NamingTokens, BoxError>;
}

#[async_trait]
impl<T> DynMediaModule for T
where
    T: MediaModule,
{
    fn media_type(&self) -> MediaType {
        MediaModule::media_type(self)
    }

    async fn search_terms(&self, content: &ContentRef) -> Result<SearchTerms, BoxError> {
        MediaModule::search_terms(self, content)
            .await
            .map_err(|e| Box::new(e) as BoxError)
    }

    async fn match_release(&self, parsed: &ParsedRelease) -> Result<Vec<ContentMatch>, BoxError> {
        MediaModule::match_release(self, parsed)
            .await
            .map_err(|e| Box::new(e) as BoxError)
    }

    async fn naming_tokens(&self, content: &ContentRef) -> Result<NamingTokens, BoxError> {
        MediaModule::naming_tokens(self, content)
            .await
            .map_err(|e| Box::new(e) as BoxError)
    }
}

/// A registry of media modules keyed by [`MediaType`].
///
/// Constructed once at startup with the modules a deployment enables, then shared
/// (it is `Send + Sync`). Adding a media type is registering one more module —
/// no pipeline change, per the spec's "adding a media type = a new module".
#[derive(Default)]
pub struct MediaRegistry {
    modules: HashMap<MediaType, Box<dyn DynMediaModule>>,
}

impl MediaRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
        }
    }

    /// Register `module` for its [`MediaModule::media_type`], replacing any
    /// previously registered module for that type.
    pub fn register<T: MediaModule + 'static>(&mut self, module: T) {
        let media_type = MediaModule::media_type(&module);
        self.modules.insert(media_type, Box::new(module));
    }

    /// The module registered for `media_type`, if any.
    #[must_use]
    pub fn get(&self, media_type: MediaType) -> Option<&dyn DynMediaModule> {
        self.modules.get(&media_type).map(AsRef::as_ref)
    }

    /// The media types that have a registered module.
    pub fn media_types(&self) -> impl Iterator<Item = MediaType> + '_ {
        self.modules.keys().copied()
    }
}
