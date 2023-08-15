# syncavify
Sync Cava colors to Spotify album art.

## For All Users
This project changes colors for the Cava Terminal visualizer. Instructions for installing Cava are available on the [project's GitHub](https://github.com/karlstav/cava).

1. Go to the `.env.template` file in this project and change it to `.env`. If you're using linux you can remove all lines in this file except for `CAVA_CONFIG_LOCATION`.
2. Find your Cava config (by default `~/.config/cava/config`) and modify the `CAVA_CONFIG_LOCATION` env var to point at the config.
3. Follow your OS-specific instructions below
4. Use `cargo run` to run this program (you will need to run `cava` in a separate window, and have Spotify running for anything to happen. Will not open either automatically).

## For Linux Users
Make sure you have `playerctl` installed. `playerctl` [(repo)](https://github.com/altdesktop/playerctl) is a CLI tool that implements [MPRIS](https://specifications.freedesktop.org/mpris-spec/latest/) and makes for faster and less resource-intensive polling for the album art.


## For Non-Linux Users
Without a handy utility availble like `playerctl`, installation is a little more involved. There is some AppleScripts magic that you can use to automate this on OSX, but for now
we can just auth with the Spotify client directly (even if it is a bit cumbersome). I use a Linux box, so this flow is a bit clunky, but I might smooth this process out later.

1. Grab a set of Spotify API credentials (either get them from me (Kanyes) or [use your own](https://developer.spotify.com/dashboard/create)).
2. Put these API keys into your `.env` files under the appropriate environment variables
3. When you run the program, you will be taken to your browser and asked to authenticate with Spotify. Once you do so, copy and paste the URL you are taken to (will be an error page) and paste it back into your terminal. As of right now there's no refresh-token persistence, so you'll have to do this copy-pasting every time you restart the app.

https://github.com/kanyesthaker/syncavify/assets/31911175/09361518-09eb-43c3-ab3b-1eaeef9fc1c2

