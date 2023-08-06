use std::{fs, rc::Rc, process::Stdio, env, cmp::Ordering};

use dotenv::dotenv;
use image::{DynamicImage, ImageError};
use imagequant::{self, Attributes, RGBA, liq_error};
use regex::Regex;
use reqwest::Error;
use tokio::{fs::File, io::AsyncWriteExt, process::Command, time::sleep, time::Duration};
use rspotify::{
    prelude::*,
    scopes, AuthCodeSpotify, Credentials, OAuth, model::Image, Config,
};

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

#[derive(Debug)]
enum QuantizationError {
    LiqErr(liq_error),
}

fn brightness(color: RGBA) -> f64 {
    // https://alienryderflex.com/hsp.html
    (f64::powf(color.r as f64, 2.0) * 0.299) + (f64::powf(color.g as f64, 2.0) * 0.587) + (f64::powf(color.b as f64, 2.0) * 0.114).sqrt()
}


async fn image_quantizer(image: DynamicImage, width: u32, height: u32, num_colors: i32) -> Result<Vec<String>, QuantizationError> {
    // Convert image to RGBA8
    let img_rgba8: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> = image.to_rgba8();
    let mut attr: Attributes = Attributes::new();
    let bmp = img_rgba8
        .pixels()
        .map(|p| RGBA::new(p.0[0], p.0[1], p.0[2], p.0[3]))
        .collect::<Vec<RGBA<>>>()
        .into_boxed_slice();

    // Quantize image down to num_colors colors
    let usize_width: usize = usize::try_from(width).unwrap();
    let usize_height: usize = usize::try_from(height).unwrap();
    attr.set_max_colors(num_colors);
    let image = attr.new_image(&bmp, usize_width, usize_height, 0.0).map_err(QuantizationError::LiqErr)?;
    let mut quantization_result = attr.quantize(&image).map_err(QuantizationError::LiqErr)?;
    let mut palette = quantization_result.palette();

    palette.sort_by(|a, b| {
        let luma_a = brightness(*a);
        let luma_b = brightness(*b);
        luma_a.partial_cmp(&luma_b).unwrap_or(Ordering::Equal)
    });
    // Convert RGBA into hex and return in a Vec
    let top_color_strings: Vec<String> = palette
        .iter()
        .map(|&color| format!("{:02X}{:02X}{:02X}", color.r, color.g, color.b))
        .collect();

    Ok(top_color_strings)
}


#[derive(Debug)]
enum ImageIngestError {
    DownloadErr(Error),
    ImageErr(ImageError),
}

async fn download_img(url: String) -> Result<DynamicImage, ImageIngestError> {
    // Download the image into bytes and return an image::DynamicImage
    let response: reqwest::Response = reqwest::get(url).await.map_err(ImageIngestError::DownloadErr)?;
    let buf = response.bytes().await.map_err(ImageIngestError::DownloadErr)?;

    let image: DynamicImage = image::load_from_memory(&buf).map_err(ImageIngestError::ImageErr)?;
    Ok(image)
}

async fn update_cava_colors(config: &str, bg_color: &String, grad_1: &String, grad_2: &String) -> Result<(), std::io::Error> {
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

async fn get_url_playerctl() -> String {
    let cmd = "playerctl metadata mpris:artUrl 2>/dev/null | sed s/open.spotify.com/i.scdn.co/";
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdout(Stdio::piped())
        .output().await;

    match out {
        Ok(result) => String::from_utf8_lossy(&result.stdout).to_string(),
        Err(e) => panic!("Failed to get image {e:?}")
    }
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

async fn image_pipeline(url: String) {
    let image: DynamicImage = match download_img(url).await { 
        Ok(result) => result,
        Err(e) => panic!("Image Ingestion Error! {e:?}")
    };
    let width = image.width();
    let height: u32 = image.height();
    let top_colors: Vec<String> = match image_quantizer(image, width, height, 3).await {
        Ok(result) => result,
        Err(e) => panic!("Image Quantization Error! {e:?}")
    };

    let config_path = env::var("CAVA_CONFIG_LOCATION").expect("No config found").to_string();
    let _ = update_cava_colors(&config_path, &top_colors[0], &top_colors[1], &top_colors[2]).await;
    let _ = reload_cava().await;
}

#[tokio::main]
async fn main() {
    env_logger::init();
    dotenv().ok();
    let mut image_url: Rc<String> = Rc::new(String::new());
    if env::consts::OS == "linux" {
        loop {
            let next_url = get_url_playerctl().await; 
            if next_url != *image_url {
                image_url = Rc::new(next_url);
                let _ = image_pipeline(image_url.to_string()).await;
            }
        }
    } else {
        let spotify = auth_spotify().await;
        let poll_delay = Duration::from_secs(1);
        loop {
            if let Some(next_url) = get_smallest_img_url(&spotify).await {
                if next_url != *image_url {
                    image_url = Rc::new(next_url);
                    let _ = image_pipeline(image_url.to_string()).await;
                }
            }
            let _ = sleep(poll_delay).await;
        }
    }
}