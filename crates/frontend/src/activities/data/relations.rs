//! Explicit catalog refresh shared by dataset views.
pub(crate) struct CatalogRefresh;
impl CatalogRefresh {
    pub(super) fn request() {
        crate::client::queries::QueryRefresh::current()
            .invalidate(&[crate::client::queries::QueryDomain::Catalog]);
    }
}
