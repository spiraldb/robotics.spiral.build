//! Settings reads belong to the mounted Settings view.
use super::SettingsClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use dioxus::prelude::*;
use spiral_robotics_common::settings::DataSettings;
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SettingsResource {
    connection: Resource<
        Result<spiral_robotics_common::settings::SpiralSettings, crate::client::CommandError>,
    >,
    settings: Resource<Result<DataSettings, String>>,
    loaded_settings: Signal<Option<DataSettings>>,
}

impl SettingsResource {
    pub(super) fn use_resource(active: ReadSignal<bool>) -> Self {
        let connection = QueryPolicy::on_demand(QueryDomain::Settings).use_when(
            move || *active.read(),
            || SettingsClient::current().connection_settings(),
        );
        let settings = QueryPolicy::new(QueryDomain::Settings, 10_000).use_when(
            move || *active.read(),
            || async {
                SettingsClient::current()
                    .data_settings()
                    .await
                    .map_err(|error| error.to_string())
            },
        );
        let mut loaded_settings = use_signal(|| None::<DataSettings>);
        use_effect(move || {
            let result = settings.read();
            if let Some(Ok(value)) = result.as_ref()
                && loaded_settings.peek().as_ref() != Some(value)
            {
                loaded_settings.set(Some(value.clone()));
            }
        });
        Self {
            connection,
            settings,
            loaded_settings,
        }
    }

    pub(super) fn loaded(self) -> Option<DataSettings> {
        self.loaded_settings.read().clone().or_else(|| {
            self.connection
                .read()
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|connection| {
                    DataSettings::new(connection.clone(), Default::default(), Default::default())
                })
        })
    }
    pub(super) fn preferences_loaded(self) -> bool {
        self.loaded_settings.read().is_some()
    }
    pub(super) fn error(self) -> Option<String> {
        self.settings
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().err())
            .cloned()
    }
    pub(super) fn refresh(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Settings]);
    }
}
