use anyhow::Result;
use std::{fs, process::Stdio, env::{self, consts::OS}, cmp::Ordering};

use dotenv::dotenv;
use image::DynamicImage;
use imagequant::{self, RGBA};
use itertools::Itertools;
use regex::Regex;
use tokio::{fs::File, io::AsyncWriteExt, process::Command};
use rspotify::{
    prelude::*,
    scopes, AuthCodeSpotify, Credentials, OAuth, model::Image, Config,
};

const DEFAULT_IMAGE_URL: &str = "https://placehold.co/600x400";

async fn get_smallest_img_url(spotify: &AuthCodeSpotify) -> Option<String> {
    // Get the currently playing song
    let response: Option<rspotify::model::CurrentlyPlayingContext> = spotify.current_user_playing_item().await.unwrap();


    if let Some(context) = response {
        // If a song is playing, grab the album/episode images and return the image with smallest size
        let playable_item: rspotify::model::PlayableItem = context.item.unwrap();
        let images: Vec<Image> = match playable_item {
            rspotify::model::PlayableItem::Track(track) => {
                track.album.images
            }
            rspotify::model::PlayableItem::Episode(episode) => {
                episode.images
            }
        };
        let smallest_image: String = images
            .iter()
            .filter_map(|image: &Image| image.height.map(|height: u32| (height, image.url.clone())))
            .min_by(|(height1, _url1), (height2, _url2)| height1.cmp(height2)).unwrap().1;
        Some(smallest_image)
    } else {
        None
    }
}

struct CavaColors<'a>(&'a String, &'a String, &'a String);

async fn update_cava_colors(config: &str, CavaColors(ref bg_color, ref grad_1, ref grad_2): CavaColors<'_>) -> Result<()> {
    // HACK uses regex replacement to swap out the colors in the config, note this is not very customizable at the moment
    let config_str = fs::read_to_string(config)?;

    // Replace the relevant config lines with regex
    let background_rgx = Regex::new(r#"background\s*=\s*'#([0-9A-Fa-f]{6})'"#).unwrap();
    let gradient_1_rgx = Regex::new(r#"gradient_color_1\s*=\s*'#([0-9A-Fa-f]{6})'"#).unwrap();
    let gradient_2_rgx = Regex::new(r#"gradient_color_2\s*=\s*'#([0-9A-Fa-f]{6})'"#).unwrap();

    let background_replace = background_rgx.replace(&config_str, &format!("background = '#{}'", bg_color));
    let gradient_1_replace = gradient_1_rgx.replace(&background_replace, &format!("gradient_color_1 = '#{}'", grad_1));
    let gradient_2_replace = gradient_2_rgx.replace(&gradient_1_replace, &format!("gradient_color_2 = '#{}'", grad_2));

    let mut file = File::create(config).await?;
    file.write_all(gradient_2_replace.as_bytes()).await?;
    Ok(())
}


async fn reload_cava() -> std::io::Result<()> {
    // Send a signal to Cava to reload its color configuration only
    // See cava docs https://github.com/karlstav/cava
    let cmd: &str = "pkill -USR2 -x cava";
    let _ = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .spawn()
        .expect("Failed to start")
        .wait()
        .await
        .expect("Failed to run");

    Ok(())
}

async fn auth_spotify() -> AuthCodeSpotify {
    let creds: Credentials = Credentials::from_env().unwrap();
    let oauth: OAuth = OAuth::from_env(scopes!("user-read-currently-playing")).unwrap();
    let config: Config = Config {
        ..Default::default()
    };

    let spotify: AuthCodeSpotify = AuthCodeSpotify::with_config(creds, oauth, config);

    let auth_url: String = spotify.get_authorize_url(false).unwrap();
    spotify.prompt_for_token(&auth_url).await.expect("Authentication Failed");
    spotify
}

struct CavaImage {
    url: String,
    num_quantization_colors: i32,
}

impl CavaImage {
    fn new(num_colors: i32) -> Self {
        Self {
            url: DEFAULT_IMAGE_URL.to_owned(),
            num_quantization_colors: num_colors,
        }
    }

    async fn do_dbus_loop(&mut self) {
        loop {
            let next_url = get_url_playerctl().await;
            if *next_url != *self.url {
                self.url = next_url;
                self.image_pipeline().await;
            }
        }
    }

    async fn image_pipeline(&self) {
        let image = download_img(&self.url)
            .await
            .expect("Image ingestion error");

        let colors = image_quantizer(image, self.num_quantization_colors).await.unwrap();
        let config_path = env::var("CAVA_CONFIG_LOCATION").unwrap_or(String::from(".config/cava/config"));
        let _ = update_cava_colors(&config_path, CavaColors(&colors[0], &colors[1], &colors[2])).await;
        let _ = reload_cava().await;
    }
}

async fn get_url_playerctl() -> String {
    let cmd = "playerctl metadata mpris:artUrl 2>/dev/null | sed s/open.spotify.com/i.scdn.co/";
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdout(Stdio::piped())
        .output().await;

    String::from_utf8_lossy(&out.ok().unwrap().stdout).to_string()
}

async fn download_img(url: &String) -> Result<DynamicImage> {
    let image_buffer = reqwest::get(url)
        .await?
        .bytes()
        .await?;
    Ok(image::load_from_memory(&image_buffer)?)
}

fn brightness(color: RGBA) -> f64 {
    // https://alienryderflex.com/hsp.html
    (f64::powf(color.r as f64, 2.0) * 0.299)
    + (f64::powf(color.g as f64, 2.0) * 0.587)
    + (f64::powf(color.b as f64, 2.0) * 0.114).sqrt()
}

async fn image_quantizer(image: DynamicImage, num_colors: i32) -> Result<Vec<String>> {
    let mut attr = imagequant::Attributes::new();
    attr.set_max_colors(num_colors);
    let bmp = image
        .to_rgba8()
        .pixels()
        .map(|p| imagequant::RGBA::new(p.0[0], p.0[1], p.0[2], p.0[3]))
        .collect::<Vec<RGBA>>();
    let image = attr.new_image(
        &bmp,
        image.width() as usize,
        image.height() as usize,
        0.0)?;
    let palette = attr
        .quantize(&image)?
        .palette()
        .into_iter()
        .sorted_by(|a, b| {
            brightness(*a).partial_cmp(&brightness(*b)).unwrap_or(Ordering::Equal)
        })
        .map(|color| format!("{:02X}{:02X}{:02X}", color.r, color.g, color.b))
        .collect();

    Ok(palette)
}

#[tokio::main]
async fn main() {
    // env_logger::init();
    dotenv().ok();
    let mut cava = CavaImage::new(3);
    if OS == "linux" { cava.do_dbus_loop().await } else { println!("Whoops!") }
    
    //     let spotify = auth_spotify().await;
    //     let poll_delay = Duration::from_secs(1);
    //     loop {
    //         if let Some(next_url) = get_smallest_img_url(&spotify).await {
    //             if next_url != *image_url {
    //                 image_url = Rc::new(next_url);
    //                 let _ = image_pipeline(image_url.to_string()).await;
    //             }
    //         }
    //         let _ = sleep(poll_delay).await;
    //     }
    // }
}