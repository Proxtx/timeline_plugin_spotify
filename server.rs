use {
    crate::{
        config::Config,
        db::{Database, Event},
        PluginData,
    },
    chrono::Utc,
    futures::StreamExt,
    rocket::{
        http::{Status},
        get, routes,
        serde::json::Json,
        State,
        fs::NamedFile
    },
    serde::{Deserialize, Serialize},
    std::{path::PathBuf, sync::Arc},
    types::{
        api::{APIResult, APIError, AvailablePlugins, CompressedEvent},
        timing::Timing,
    },
};

pub struct Plugin {
    plugin_data: PluginData,
}

impl crate::Plugin for Plugin {
    async fn new(data: crate::PluginData) -> Self
    where
        Self: Sized,
    {
        Plugin { plugin_data: data }
    }

    fn get_type() -> types::api::AvailablePlugins
    where
        Self: Sized,
    {
        types::api::AvailablePlugins::timeline_plugin_notification
    }

    fn get_routes() -> Vec<rocket::Route>
    where
        Self: Sized,
    {
        routes![new_notification, app_icon]
    }

    fn get_compressed_events(
        &self,
        query_range: &types::timing::TimeRange,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Future<Output = types::api::APIResult<Vec<types::api::CompressedEvent>>>
                + Send,
        >,
    > {
        let filter = Database::generate_range_filter(query_range);
        let plg_filter = Database::generate_find_plugin_filter(AvailablePlugins::timeline_plugin_notification);
        let filter = Database::combine_documents(filter, plg_filter);
        let database = self.plugin_data.database.clone();
        Box::pin(async move {
            let mut cursor = database
                .get_events::<Notification>()
                .find(filter, None)
                .await?;
            let mut result = Vec::new();
            while let Some(v) = cursor.next().await {
                let t = v?;
                result.push(CompressedEvent {
                    title: t.event.app.clone(),
                    time: t.timing,
                    data: Box::new(t.event),
                })
            }

            Ok(result)
        })
    }
}

#[derive(Deserialize, Serialize)]
struct Notification {
    app: String,
    title: String,
    content: String,
}

#[get("/notification/<password>/<app>/<title>/<content>")]
async fn new_notification(
    password: &str,
    app: &str,
    title: &str,
    content: &str,
    config: &State<Config>,
    database: &State<Arc<Database>>,
) -> (Status, Json<APIResult<()>>) {
    if password != config.password {
        return (Status::Unauthorized, Json(Err(APIError::AuthenticationError)));
    }

    match database
        .register_single_event(&Event {
            timing: Timing::Instant(Utc::now()),
            id: Utc::now().timestamp_millis().to_string(),
            plugin: AvailablePlugins::timeline_plugin_notification,
            event: Notification {
                app: app.to_string(),
                title: title.to_string(),
                content: content.to_string()
            },
        })
        .await
    {
        Ok(_) => (Status::Ok, Json(Ok(()))),
        Err(e) => {
            crate::error::error(database.inner().clone(), &e, Some(<Plugin as crate::Plugin>::get_type()));
            (Status::InternalServerError, Json(Err(e.into())))
        },
    }
}

#[get("/icon/<app>")]
pub async fn app_icon(app: &str) -> Option<NamedFile> {
    let mut path = PathBuf::from("../plugins/timeline_plugin_notification/icons/");
    path.push(app.to_lowercase());
    NamedFile::open(path).await.ok()
}
