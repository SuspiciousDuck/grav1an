# grav1an
`grav1an` is a binary which creates psychovisually optimized AV1 video using [Vapoursynth](https://github.com/vapoursynth/vapoursynth), [Av1an](https://github.com/master-of-zen/Av1an), [Grav1synth](https://github.com/rust-av/grav1synth), and [MKVToolNix](https://github.com/nmaier/mkvtoolnix). Currently, it supports the encoders [svt-av1-psy](https://github.com/gianni-rosato/svt-av1-psy) and [rav1e](https://github.com/xiph/rav1e). This was originally a Python script made by Ironclad, so credits to them for creating the original script. The [AV1 anime server](https://discord.gg/83dRFDFDp7) has the original script & support.

## Dependencies:
Bolded dependencies are **required**.
* **[FFmpeg/FFprobe](https://ffmpeg.org)** (enable x264 to detect offsets)
* **[Vapoursynth](https://github.com/vapoursynth/vapoursynth)**
* **[Av1an](https://github.com/master-of-zen/Av1an)**
* **[svt-av1-psy](https://github.com/gianni-rosato/svt-av1-psy)/[rav1e](https://github.com/xiph/rav1e)** (at least one is required)
* **[MKVToolNix](https://mkvtoolnix.download)**
* [Grav1synth](https://github.com/rust-av/grav1synth) (required if `--no-grain` is unset)
* [opus-tools](https://github.com/xiph/opus-tools) (required for encoding opus)
* **[BestSource](https://github.com/vapoursynth/bestsource)/[LSMASHSource](https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works)/[dgdecnv](https://www.rationalqm.us/dgdecnv/dgdecnv.html)** (at least one required)
* [vs-mlrt](https://github.com/AmusementClub/vs-mlrt) (required for scaling)
### Make sure that all dependencies are in your PATH environment variable.
## Installing:
1. Install Cargo if you haven't already.
2. Clone & enter this repo
```
git clone --depth 1 --single-branch https://github.com/SuspiciousDuck/grav1an
cd grav1an
```
3. Build/Install
```
cargo install --path .
```
4. Profit
```
# Add $HOME/.cargo/bin to your PATH if you haven't already
grav1an
```
## Usage:
Basic Example:
```
# This uses the directories ./show & ./show_out as the input & output directories.
# -w specifies the amount of workers to use when encoding (not threads).
# -n specifies the name of the show, which is used when creating output files.
# --no-torrent specifies that a resulting .torrent file shouldn't be made.
# This will encode 4 fast passes in order to target a SSIMULACRA2 score of 80 in the final encode!
grav1an -i ./show -o ./show_out -n Show -w 4 --no-torrent
```