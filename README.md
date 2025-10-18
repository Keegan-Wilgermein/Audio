# Audio
System wide audio recording, editing, and playback

## Disclaimer
Don't use this application to distibute copyrighted material

## Install instructions
- Get slint from [slint.dev](https://slint.dev/checkout?interval=month&plan=royalty-free) using the royalty free plan
- Install rustup and cargo from [rust-lang.org](https://rust-lang.org/tools/install/)
- Install the slint extension for vs code
- Install the rust analyser extension for vs code
- Install the codelldb extension for vs code
- Navigate to the folder containing the project
- Paste the following text into your cargo.toml file:
```
[dependencies]
hound = "3.5.1"
kira = "0.10.8"
qruhear = "0.1.1"
rand = "0.9.2"
savefile = "0.19.0"
savefile-derive = "0.19.0"
slint = "1.12.1"

[build-dependencies]
slint-build = "1.12.1"
```
- Run ```cargo run --release```

If this does not work then:
- Follow all the previous installation steps
- Create a new slint project using the rust template
- Paste the above dependencies into the cargo.toml file
- Paste the code from the repo directly into your own files and run the project
- Don't forget to download the images

## How to use
### Recording Audio
- Click the red circle icon to start and stop recording
### Audio Playback
- Select a recording to play from the list
- Click the red play button to start playback
- Click the red pause button to stop playback
- Click the rewind button to skip to the previous track
- Click the next button to skip to the next track

Recordings can't be played while a recording is in progress

### Recording inputs
- Click the blue circle icon to start and stop recording the edits you make to the dials
### Input Playback
- Select a recording to play from the list
- Click the blue play button to start playing back your captured inputs
- Click the blue pause button to stop playback
- Click the rewind button to skip to the previous track
- Click the next button to skip to the next track
### Dials
Dials are used to adjust the way each recording sounds. Each recording saves it's own individual settings which can be saved to a preset

Rotate each dial by clicking and dragging left or right to increase or decrease the value

The avaliable dials can adjust the
- Bass
- Vocals
- Treble
- Gain
- Reverb
- Crush

Dials can't be rotated while recording new audio or playing back captured inputs
### Presets
Presets allow you to save settings to be quickly applied to other recordings
- Click the plus icon next to the presets list to save a preset
- Select a preset in the list to apply its settings to a recording
### Deleting presets and recordings
- Click the respective trash icon in each list
- Select the preset or recording you want to delete
- Click the respective check icon when done
### Renaming presets and recorings
- Click the respective pen icon in each list
- Select the preset or recording you want to delete
- Click the respective check icon when done