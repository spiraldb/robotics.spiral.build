//! The joint roles the browser resolved for each dataset version.
use sea_orm::entity::prelude::*;
use sea_orm::{Set, sea_query::OnConflict};
use spiral_robotics_common::datasets::DatasetJointRoles;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "dataset_joint_roles")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) project: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub(super) dataset: String,
    pub(super) version: String,
    pub(super) document: Json,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}

impl Model {
    pub(super) fn roles(self) -> Result<DatasetJointRoles, String> {
        let roles: DatasetJointRoles =
            serde_json::from_value(self.document).map_err(|error| error.to_string())?;
        roles.validate()?;
        Ok(roles)
    }
}

impl super::StudioProject {
    /// The roles resolved for dataset version `identity`; `None` until they are.
    pub(crate) async fn dataset_joint_roles(
        &self,
        identity: (&str, &str),
    ) -> Result<Option<DatasetJointRoles>, String> {
        Entity::find_by_id((self.key().to_owned(), identity.0.to_owned()))
            .filter(Column::Version.eq(identity.1))
            .one(self.connection().await?)
            .await
            .map_err(|error| error.to_string())?
            .map(Model::roles)
            .transpose()
    }
    pub(crate) async fn save_dataset_joint_roles(
        &self,
        identity: (&str, &str),
        roles: &DatasetJointRoles,
    ) -> Result<(), String> {
        roles.validate()?;
        Entity::insert(ActiveModel {
            project: Set(self.key().to_owned()),
            dataset: Set(identity.0.to_owned()),
            version: Set(identity.1.to_owned()),
            document: Set(serde_json::to_value(roles).map_err(|error| error.to_string())?),
        })
        .on_conflict(
            OnConflict::columns([Column::Project, Column::Dataset])
                .update_columns([Column::Version, Column::Document])
                .to_owned(),
        )
        .exec(self.connection().await?)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}
