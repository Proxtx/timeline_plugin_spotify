use {
    crate::{
        cache::Cache,db::{Database, Event}, Plugin as _, PluginData
    }, base64::Engine, chrono::Utc, futures::StreamExt, mongodb::{bson::doc, options::FindOneOptions}, rocket::{get, routes, State}, serde::{Deserialize, Serialize}, std::sync::Arc, tokio::sync::RwLock, types::{
        api::CompressedEvent,
        timing::Timing,
    }, url::Url
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

#[allow(unused)]
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
        let filter = Database::combine_documents(Database::generate_find_plugin_filter(Plugin::get_type()), Database::combine_documents(Database::generate_range_filter(query_range), doc! {
                "event.event_type": "Song"
        }));
        let database = self.plugin_data.database.clone();
        Box::pin(async move { 
            let mut cursor = database
                .get_events::<Song>()
                .find(filter, None)
                .await?;
            let mut result = Vec::new();
            while let Some(v) = cursor.next().await {
                let t = v?;
                result.push(CompressedEvent {
                    title: t.event.name.clone(),
                    time: t.timing,
                    data: Box::new(t.event),
                })
            }

            Ok(result)
        })
    }

    fn request_loop<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn futures::Future<Output = Option<chrono::Duration>> + Send + 'a>> {
        Box::pin(async move {
            match self.save_current_song().await {
                Ok(_) => {

                },
                Err(e) => {
                    self.plugin_data.report_error_string(e)
                }
            }
            Some(chrono::TimeDelta::try_seconds(30).unwrap())
        })
    }

    fn get_routes() -> Vec<rocket::Route>
        where
            Self: Sized, {
        routes![get_cover]
    }
}

#[get("/<url>")]
async fn get_cover(database: &State<Arc<Database>>, url: &str) -> Option<Vec<u8>> {
    match database.get_events::<ImageData>().find_one(Database::combine_documents(Database::generate_find_plugin_filter(Plugin::get_type()), doc! {
        "event.event_type": "ImageData",
        "event.url": url
    }), None).await {
        Ok(Some(v)) => {
            Some(base64::prelude::BASE64_STANDARD.decode(v.event.data).unwrap())
        }
        _ => {
            None
        }
    }
}

impl Plugin {
    async fn save_current_song (&self) -> Result<(), String> {
        let song = match self.get_current_song().await? {
            Some(v) => v,
            None => {
                return Ok(());
            }
        };
        let last_song = self.plugin_data.database.get_events::<Song>().find_one(Database::combine_documents(Database::generate_find_plugin_filter(Plugin::get_type()), doc! {"event.event_type": "Song"}), FindOneOptions::builder().sort(doc! {
            "timing.0": -1
        }).build()).await;
        let is_equal_to_last_song = match last_song {
            Ok(v) => match v {
                Some(v) => {
                    v.event.id == song.id
                }
                None => false
            }
            Err(e) => {
                return Err(format!("Unable to find last song: {}", e));
            }
        };
        if is_equal_to_last_song {
            return Ok(());
        }

        let cnt = match self.plugin_data.database.get_events::<ImageData>().count_documents(Database::combine_documents(Database::generate_find_plugin_filter(Plugin::get_type()), doc! {
            "event.url": song.image.to_string(),
            "event.event_type": "ImageData"
        }), None).await {
            Ok(v) => v,
            Err(e) => {
                return Err(format!("Unable to check if album cover is present: {}", e));
            }
        };

        let timing = Timing::Instant(Utc::now());
        
        if cnt < 1 {
            let buffer = match reqwest::get(song.image.to_string()).await {
                Ok(v) => {
                    match v.bytes().await {
                        Ok(v) => {
                            base64::prelude::BASE64_STANDARD.encode(v)
                        }
                        Err(e) => {
                            return Err(format!("Unable to read album cover response: {}", e));
                        }
                    }
                }
                Err(e) => {
                    return Err(format!("Unable to fetch album cover: {}", e));
                }
            };
            match self.plugin_data.database.register_single_event(&Event { timing: timing.clone(), id: song.image.to_string(), plugin: Plugin::get_type(), event: ImageData {
                data: buffer,
                url: song.image.to_string(),
                event_type: PluginEventType::ImageData
            }}).await {
                Ok(_v) => {},
                Err(e) => {
                    return Err(format!("Unable to register database event: {}", e));
                }
            }
        };

        match self.plugin_data.database.register_single_event(&Event { id: format!("{}@{}", song.id, serde_json::to_string(&timing).unwrap()), timing, plugin: Plugin::get_type(), event: song }).await {
            Ok(_v) => {

            },
            Err(e) => {
                return Err(format!("Unable to register song to database: {}", e));
            }
        }

        Ok(())
    }

    async fn get_current_song (&self) -> Result<Option<Song>, String> {
        let url = "https://api.spotify.com/v1/me/player";
        let mut access_token;
        {
            access_token = self.cache.read().await.get().access_token.clone();
        }

        if access_token == String::default() {
            access_token = "update_access_token".to_string();
        }

        let client = reqwest::Client::new();
        let res = match client.get(url).header("Authorization", format!("Bearer {}", access_token)).send().await {
            Ok(v) => v,
            Err(e) => {
                return Err(format!("Unable to perform spotify player request: {}", &e));
            }
        };

        let status = res.status();

        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if status != reqwest::StatusCode::OK {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                let _e = match res.text().await {
                    Ok(v) => {
                        v
                    }
                    Err(e) => {
                        format!("Unable to read request text: {}", e)
                    }
                };
                if let Err(e) = self.update_access_token().await {
                    return Err(format!("Unable to update access token: {}", e))
                }
                //return Err(format!("Unable to fetch spotify song. Refreshing Access Token: {}", e))
                return Ok(None)
            }
            return Err("Spotify API answered with an unknown http code".to_string());
        }

        let text = match res.text().await {
            Ok(v) => v,
            Err(e) => {
                return Err(format!("Unable to read text from API Request: {}", &e));
            }
        };

        let mut res: PlayerData = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                return Err(format!("Unable to parse spotify API response: {}", &e));
            }
        };

        res.item.album.images.sort_by(|a,b| b.height.cmp(&a.height));

        Ok(Some(Song {
            name: res.item.name,
            artist: res.item.artists.into_iter().map(|v| {v.name}).collect::<Vec<_>>().join(", "),
            image: res.item.album.images.remove(0).url,
            id: res.item.id,
            event_type: PluginEventType::Song
        }))
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
                self.plugin_data.report_error(&e);
                return Ok(());
            }
        };

        {
            self.cache.write().await.modify::<Plugin>(|v| v.access_token = token.access_token).expect("Unable to write to cache. Very unlikely that this is our doing");
        }

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
enum PluginEventType {
    Song,
    ImageData
}

#[derive(Deserialize, Serialize)]
struct ImageData {
    pub url: String,
    pub data: String,
    pub event_type: PluginEventType
}

#[derive(Debug, Serialize, Deserialize)]
struct Song {
    pub name: String,
    pub artist: String, 
    pub image: Url,
    pub id: String,
    pub event_type: PluginEventType
}

#[derive(Deserialize)]
struct AccessTokenRequset {
    access_token: String
}

#[derive(Deserialize)]
struct PlayerData {
    item: PlayerDataItem
}

#[derive(Deserialize)]
struct PlayerDataItem {
    id: String,
    name: String,
    artists: Vec<PlayerDataArtist>,
    album: PlayerDataAlbum
}

#[derive(Deserialize)]
struct PlayerDataArtist {
    name: String
}

#[derive(Deserialize)]
struct PlayerDataAlbum {
    images: Vec<PlayerDataImage>
}

#[derive(Deserialize)]
struct PlayerDataImage {
    height: u64,
    url: Url
}