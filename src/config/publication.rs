/// How hosted reviews (GitHub Action and App) reach the change request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicationConfig {
    /// When true, every hosted review is prepared as a candidate and is
    /// published only after a separate approval naming its digest (the
    /// Action's protected `cururu-publish` Environment, the App's check
    /// button or `/cururu approve`, or `cururu approve-hosted-review`).
    /// Off by default: the review is published as soon as it completes.
    /// Read from the trusted base configuration, so a change request cannot
    /// turn it off for itself.
    pub require_approval: bool,
}
