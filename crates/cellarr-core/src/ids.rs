//! Strongly-typed identifiers.
//!
//! Every structural entity gets its own newtype around a [`Uuid`] so that, for
//! example, a [`ContentId`] can never be passed where a [`LibraryId`] is
//! expected. They serialize transparently as the underlying UUID.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generate a fresh random identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_newtype!(
    /// Identifies a [`crate::Library`].
    LibraryId
);
id_newtype!(
    /// Identifies a structural `content` node.
    ContentId
);
id_newtype!(
    /// Identifies a physical media file.
    MediaFileId
);
id_newtype!(
    /// Identifies a subtitle sidecar fetched for a media file.
    SubtitleId
);
id_newtype!(
    /// Identifies a typed identity/metadata row (the `title_id`).
    TitleId
);
id_newtype!(
    /// Identifies a `grab` (a release handed to a download client).
    GrabId
);
id_newtype!(
    /// Identifies a configured indexer.
    IndexerId
);
id_newtype!(
    /// Identifies a configured download client.
    DownloadClientId
);
id_newtype!(
    /// Identifies a quality profile.
    QualityProfileId
);
id_newtype!(
    /// Identifies a custom format.
    CustomFormatId
);
id_newtype!(
    /// Identifies a delay profile.
    DelayProfileId
);
id_newtype!(
    /// Identifies a release profile.
    ReleaseProfileId
);
id_newtype!(
    /// Correlates all log/history records produced by one pipeline run.
    PipelineRunId
);
