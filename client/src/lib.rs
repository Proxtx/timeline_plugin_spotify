use { 
    client_api::{api, external::url::Url, plugin::{PluginData, PluginEventData, PluginTrait}, result::EventResult, style::Style}, leptos::{view, IntoView, View}, serde::{Deserialize, Serialize}
};

pub struct Plugin {
}

impl PluginTrait for Plugin {
    async fn new(_data: PluginData) -> Self
        where
            Self: Sized {
            Plugin {
            }
    }

    fn get_component(&self, data: PluginEventData) -> EventResult<Box<dyn FnOnce() -> leptos::View>> {
        let data = data.get_data::<Song>()?;
        Ok(Box::new(move || -> View {
            view! {
                <div style="display: flex; flex-direction: row; width: 100%; gap: calc(var(--contentSpacing) * 0.5); background-color: var(--accentColor1);align-items: start;">
                    <img
                        style="width: calc(var(--contentSpacing) * 10); aspect-ratio: 1;"
                        src=move || {
                            api::relative_url("/api/plugin/timeline_plugin_spotify/")
                                .unwrap()
                                .join(data.image.as_ref())
                                .unwrap()
                                .to_string()
                        }
                    />

                    <div style="padding-top: calc(var(--contentSpacing) * 0.5); padding-bottom: calc(var(--contentSpacing) * 0.5); color: var(--lightColor); overflow: hidden;">
                        <h3>{move || { data.name.clone() }}</h3>
                        <a>{move || { data.artist.clone() }}</a>
                    </div>
                </div>
            }.into_view()
        }))
    }

    fn get_style(&self) -> Style {
        Style::Acc1
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Song {
    pub name: String,
    pub artist: String, 
    pub image: Url,
    pub id: String,
}