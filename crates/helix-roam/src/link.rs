/// The kind of edge connecting two [`Node`](crate::Node)s.
///
/// Org-Roam records two kinds of references between nodes:
///
/// * `Id` — a direct `[[id:<uuid>][description]]` link, the canonical way
///   nodes point at each other.
/// * `Ref` — a match against a node's `:ROAM_REFS:` property, which lets a
///   node claim an external identifier (a URL, a `@citekey`, …) so that
///   citations of it resolve back to the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Link {
    /// An `[[id:<uuid>]]` link.
    Id,
    /// A reference resolved through the target's `:ROAM_REFS:` property.
    Ref,
}

impl Link {
    /// Whether this is an `[[id:…]]` link.
    pub fn is_id(self) -> bool {
        matches!(self, Link::Id)
    }

    /// Whether this edge was resolved through `:ROAM_REFS:`.
    pub fn is_ref(self) -> bool {
        matches!(self, Link::Ref)
    }
}
