use {
    crate::{
        cache::Cache, config::Config, db::{Database, Event}, error, Plugin as _, PluginData
    }, base64::Engine, chrono::Utc, futures::{task::ArcWake, StreamExt}, rocket::{fs::NamedFile, get, http::Status, routes, serde::json::Json, State}, serde::{Deserialize, Serialize}, serde_json::json, std::{f64::consts::E, path::PathBuf, sync::Arc}, tokio::sync::RwLock, types::{
        api::{APIError, APIResult, AvailablePlugins, CompressedEvent},
        timing::Timing,
    }
};

#[derive(Deserialize)]
struct ConfigData {
    client_id: String,
    client_secret: String,
    refresh_token: String
}

#[derive(Serialize, Deserialize, Default)]
struct AccessTokenCache {
    pub access_token: String
}

pub struct Plugin {
    plugin_data: PluginData,
    config: ConfigData,
    cache: RwLock<Cache<AccessTokenCache>>
}

impl crate::Plugin for Plugin {
    async fn new(data: crate::PluginData) -> Self
    where
        Self: Sized,
    {
        let config: ConfigData = toml::Value::try_into(
            data.config
                .clone().expect("Failed to init spotify plugin! No config was provided!")
                ,
        )
        .unwrap_or_else(|e| panic!("Unable to init spotify plugin! Provided config does not fit the requirements: {}", e));

    let cache: Cache<AccessTokenCache> =
            Cache::load::<Plugin>().await.unwrap_or_else(|e| {
                panic!(
                    "Failed to init media_scan plugin! Unable to load cache: {}",
                    e
                )
            });
        Plugin { plugin_data: data, config, cache: RwLock::new(cache) }
    }

    fn get_type() -> types::api::AvailablePlugins
    where
        Self: Sized,
    {
        types::api::AvailablePlugins::timeline_plugin_spotify
    }

    fn get_compressed_events(&self, query_range: &types::timing::TimeRange) -> std::pin::Pin<
        Box<
            dyn futures::Future<Output = types::api::APIResult<Vec<types::api::CompressedEvent>>>
                + Send,
        >> 
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn request_loop<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn futures::Future<Output = Option<chrono::Duration>> + Send + 'a>> {
        Box::pin(async move {
            self.update_access_token().await.unwrap();
            Some(chrono::TimeDelta::try_minutes(2).unwrap())
        })
    }
}

impl Plugin {
    async fn get_current_song (&self) {
        let url = "https://api.spotify.com/v1/me/player";
        let access_token;
        {
            access_token = self.cache.read().await.get().access_token.clone();
        }

        let client = reqwest::Client::new();
        let res = match client.get(url).header("Authorization", format!("Bearer {}", access_token)).send().await {
            Ok(v) => v,
            Err(e) => {
                error::error(self.plugin_data.database.clone(), &e, Some(Plugin::get_type()));
                return;
            }
        };

        let status = res.status();

        if status == reqwest::StatusCode::NO_CONTENT {
            return;
        }

        if status != reqwest::StatusCode::OK {
            if status == 401 {
                let e = match res.text().await {
                    Ok(v) => {
                        v
                    }
                    Err(e) => {
                        format!("Unable to read request text: {}", e)
                    }
                };
                error::error_string(self.plugin_data.database.clone(), format!("Unable to fetch spotify song: {}", e), Some(Plugin::get_type()))
            }
            if let Err(e) = self.update_access_token().await {
                error::error(self.plugin_data.database.clone(), &e, Some(Plugin::get_type()))
            }

            return;
        }

        let text = match res.text().await {
            Ok(v) => v,
            Err(e) => {
                error::error(self.plugin_data.database.clone(), &e, Some(Plugin::get_type()));
                return;
            }
        };

        //let res = 
    }

    async fn update_access_token (&self) -> Result<(), reqwest::Error> {
        let url = "https://accounts.spotify.com/api/token";

        let auth_encoded = base64::prelude::BASE64_STANDARD.encode(format!("{}:{}", self.config.client_id, self.config.client_secret));

        let client = reqwest::Client::new();
        let rqwst = client.post(url).header("Content-Type", "application/x-www-form-urlencoded").header("Authorization", format!("Basic {}", auth_encoded)).body(format!("grant_type=refresh_token&refresh_token={}", self.config.refresh_token)).send().await?;
        
        let txt = rqwst.text().await?;
        let token: AccessTokenRequset =  match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                error::error(self.plugin_data.database.clone(), &e, Some(Plugin::get_type()));
                return Ok(());
            }
        };

        {
            self.cache.write().await.modify::<Plugin>(|v| v.access_token = token.access_token).expect("Unable to write to cache. Very unlikely that this is our doing");
        }

        Ok(())
    }
}

#[derive(Deserialize)]
struct AccessTokenRequset {
    access_token: String
}