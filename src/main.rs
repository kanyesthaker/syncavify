use std::{fs, rc::Rc};

use image::{DynamicImage, ImageError};
use imagequant::{self, Attributes, RGBA, liq_error};
use regex::Regex;
use reqwest::Error;
use rspotify::{
    prelude::*,
    scopes, AuthCodeSpotify, Credentials, OAuth, model::Image, Config,
};
use tokio::{fs::File, io::AsyncWriteExt, process::Command, time::sleep, time::Duration};

struct ImageMeta {
    size: u32,
    url: String
}

async fn get_smallest_img_url(spotify: &AuthCodeSpotify) -> Option<ImageMeta> {
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
        let smallest_image: (u32, String) = images
            .iter()
            .filter_map(|image: &Image| image.height.map(|height: u32| (height, image.url.clone())))
            .min_by(|(height1, _url1), (height2, _url2)| height1.cmp(height2)).unwrap();
        Some(ImageMeta { size: smallest_image.0, url: smallest_image.1 })
    } else {
        None
    }
}

#[derive(Debug)]
enum QuantizationError {
    LiqErr(liq_error),
}

async fn image_quantizer(image: DynamicImage, size: u32, num_colors: i32) -> Result<Vec<String>, QuantizationError> {
    // Convert image to RGBA8
    let img_rgba8: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> = image.to_rgba8();
    let mut attr: Attributes = Attributes::new();
    let bmp = img_rgba8
        .pixels()
        .map(|p| RGBA::new(p.0[0], p.0[1], p.0[2], p.0[3]))
        .collect::<Vec<RGBA<>>>()
        .into_boxed_slice();

    // Quantize image down to num_colors colors
    let usize_size = usize::try_from(size).unwrap();
    attr.set_max_colors(num_colors);
    let image = attr.new_image(&bmp, usize_size, usize_size, 0.0).map_err(QuantizationError::LiqErr)?;
    let mut quantization_result = attr.quantize(&image).map_err(QuantizationError::LiqErr)?;
    let palette = quantization_result.palette();

    // Convert RGBA into hex and return in a Vec
    let top_color_strings: Vec<String> = palette
        .iter()
        .map(|&color| format!("{:02X}{:02X}{:02X}", color.r, color.g, color.b))
        .collect();
    return Ok(top_color_strings)
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
    println!("Restarting cava");
    let _ = Command::new("pkill")
        .arg("-USR2")
        .arg("cava")
        .spawn()
        .expect("Failed to reload Cava");

    Ok(())
}


#[tokio::main]
async fn main() {
    env_logger::init();

    let creds: Credentials = Credentials::from_env().unwrap();
    let oauth: OAuth = OAuth::from_env(scopes!("user-read-currently-playing")).unwrap();
    let config: Config = Config {
        ..Default::default()
    };

    let spotify: AuthCodeSpotify = AuthCodeSpotify::with_config(creds, oauth, config);

    let auth_url: String = spotify.get_authorize_url(false).unwrap();
    spotify.prompt_for_token(&auth_url).await.expect("Authentication Failed");

    let mut image_size: u32;
    let mut image_url: Rc<String> = Rc::new(String::new());

    let poll_delay = Duration::from_secs(3);
    loop {
        if let Some(image_meta) = get_smallest_img_url(&spotify).await {
            println!("Next loop started");
            if image_meta.url == *image_url {
                sleep(poll_delay).await;
                continue;
            }
            image_size = image_meta.size;
            image_url = Rc::new(image_meta.url);
        } else {
            println!("No currently playing song");
            sleep(poll_delay).await;
            continue;
        }
        println!("Smallest image url: {auth_url:?}");
        let image: DynamicImage = match download_img(image_url.to_string()).await { 
            Ok(result) => result,
            Err(e) => panic!("Image Ingestion Error! {e:?}")
        };
        let top_colors: Vec<String> = match image_quantizer(image, image_size, 3).await {
            Ok(result) => result,
            Err(e) => panic!("Image Quantization Error! {e:?}")
        };
        println!("Top colors: {top_colors:?}");
        match update_cava_colors("/home/kanyes/.config/cava/config", &top_colors[0], &top_colors[1], &top_colors[2]).await {
            Ok(result) => {},
            Err(e) => {}
        };
        reload_cava().await;
        sleep(poll_delay).await;
        println!("About to start next iteration!");
    }
}