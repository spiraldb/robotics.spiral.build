use std::sync::{Arc, Mutex};

use arrow_schema::Schema;
use futures::{FutureExt as _, future::BoxFuture};
use spiral_engine::RelationMeta;
use spiral_engine::catalog::{
    BoundRelation, Catalog, CatalogContext, RelationDefinition, RelationName,
};
use spiral_engine::logical::{Empty, LogicalPlan};
use spiral_engine::sql::{ConnectionCtx, ConnectionScopeId, TransactionCtx};
use spiral_session::SpiralSession;
use vortex_array::dtype::{DType, Nullability, PType, StructFields};
use vortex_error::{VortexResult, vortex_err};

use super::read_transaction::ReadTransactionGuard;
use super::{DatabaseResult, SpiralConnection};

#[derive(Debug)]
struct ConnectionRequiredCatalog {
    connection_id: ConnectionScopeId,
    pending_transaction: Mutex<Option<TransactionCtx>>,
}

impl Catalog for ConnectionRequiredCatalog {
    fn bind_relation<'a>(
        &'a self,
        context: CatalogContext<'a>,
        name: &'a RelationName,
    ) -> BoxFuture<'a, VortexResult<Option<BoundRelation>>> {
        async move {
            let transaction = context
                .transaction()
                .ok_or_else(|| vortex_err!("catalog read omitted the connection transaction"))?;
            assert_eq!(transaction.connection().id(), self.connection_id);
            assert_eq!(context.current_project(), Some("schema_test_project"));
            match name.dotted().to_string().as_str() {
                "missing" => return Ok(None),
                "denied" => return Err(vortex_err!("catalog access denied")),
                "pending" => {
                    if !self
                        .pending_transaction
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|pending| pending.is_same_transaction(transaction))
                    {
                        return Ok(None);
                    }
                }
                _ => {}
            }
            let dtype = DType::Struct(
                StructFields::from_iter([(
                    "id",
                    DType::Primitive(PType::I64, Nullability::NonNullable),
                )]),
                Nullability::NonNullable,
            );
            Ok(Some(BoundRelation::new(
                name.clone(),
                RelationDefinition::table(dtype.clone()),
                LogicalPlan::empty(Empty::try_new(RelationMeta::unordered(dtype))),
            )))
        }
        .boxed()
    }
}

struct SchemaFixture {
    connection: SpiralConnection,
    catalog: Arc<ConnectionRequiredCatalog>,
}

impl SchemaFixture {
    fn new() -> Self {
        let context = ConnectionCtx::new(SpiralSession::default());
        let catalog = Arc::new(ConnectionRequiredCatalog {
            connection_id: context.id(),
            pending_transaction: Mutex::new(None),
        });
        context
            .relation_catalog_mut()
            .register_catalog("schema_test", catalog.clone());
        context.use_catalog("schema_test").unwrap();
        context.use_project("schema_test_project").unwrap();
        Self {
            connection: SpiralConnection::new(context),
            catalog,
        }
    }

    async fn schema(&mut self, name: &str) -> DatabaseResult<Option<Arc<Schema>>> {
        self.connection
            .fetch_table_schema(crate::db::RelationName::parse(name).unwrap())
            .await
    }
}

#[tokio::test]
async fn connection_lookup_preserves_context_but_reports_missing_as_an_error() {
    let fixture = SchemaFixture::new();
    let present = fixture
        .connection
        .context
        .try_relation("present")
        .await
        .unwrap();
    assert_eq!(
        present.transaction().connection().id(),
        fixture.connection.context.id()
    );
    assert!(
        fixture
            .connection
            .context
            .try_relation("missing")
            .await
            .is_err()
    );
    assert!(fixture.connection.context.active_transaction().is_none());
}

#[tokio::test]
async fn schema_inspection_distinguishes_absence_from_access_failure() {
    let mut fixture = SchemaFixture::new();
    let schema = fixture.schema("present").await.unwrap().unwrap();
    assert_eq!(
        schema.field_with_name("id").unwrap().data_type(),
        &arrow_schema::DataType::Int64
    );
    assert!(fixture.schema("missing").await.unwrap().is_none());
    assert!(fixture.schema("denied").await.is_err());
    assert!(fixture.connection.context.active_transaction().is_none());
    assert!(fixture.schema("present").await.unwrap().is_some());
}

#[tokio::test]
async fn qualified_catalog_names_use_the_same_connection() {
    let mut fixture = SchemaFixture::new();
    assert!(
        fixture
            .schema("schema_test.dataset.present")
            .await
            .unwrap()
            .is_some()
    );
    assert!(fixture.connection.context.active_transaction().is_none());
}

#[tokio::test]
async fn schema_inspection_preserves_the_callers_pending_transaction() {
    let mut fixture = SchemaFixture::new();
    fixture
        .connection
        .context
        .set_autocommit(false)
        .await
        .unwrap();
    let transaction = fixture.connection.context.active_transaction().unwrap();
    *fixture.catalog.pending_transaction.lock().unwrap() = Some(transaction.clone());

    assert!(fixture.schema("pending").await.unwrap().is_some());
    assert!(fixture.schema("missing").await.unwrap().is_none());
    assert!(fixture.schema("denied").await.is_err());
    assert!(
        fixture
            .connection
            .context
            .active_transaction()
            .unwrap()
            .is_same_transaction(&transaction)
    );
    assert!(!fixture.connection.context.autocommit());
    fixture.connection.context.rollback().await.unwrap();
}

#[tokio::test]
async fn dropping_a_read_guard_closes_only_its_own_transaction() {
    let fixture = SchemaFixture::new();
    let read_transaction = ReadTransactionGuard::open(&fixture.connection.context)
        .await
        .unwrap();
    assert!(fixture.connection.context.active_transaction().is_some());
    drop(read_transaction);
    assert!(fixture.connection.context.active_transaction().is_none());

    let transaction = fixture
        .connection
        .context
        .begin_connection_transaction()
        .await
        .unwrap();
    drop(
        ReadTransactionGuard::open(&fixture.connection.context)
            .await
            .unwrap(),
    );
    assert!(
        fixture
            .connection
            .context
            .active_transaction()
            .unwrap()
            .is_same_transaction(&transaction)
    );
    fixture.connection.context.rollback_transaction().unwrap();
}
