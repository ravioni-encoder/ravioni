# ravioni

Simple AV1 encoder GUI inspired by [Aviator](https://github.com/gianni-rosato/aviator), but better
😉.

* Focus on simplicity and "it just works".
    * Advanced features can be activated manually.
* Encodes only to AV1 and Opus because these are the state-of-the-art codecs currently.
* Persistent UI. No values are erased when restarting the application. This is a major annoyance in
  many encoders, including Aviator. Ravioni saves to disk automatically after every change.
    * There is an option to not reload many settings when opening a new source file. This makes it
      easier to convert many files in a row.
* Separate bitrate settings for audio streams with different channel counts. Aviator encodes Stereo
  and 7.1 using the same bitrate.
* Automatically finds and adds subtitle files to output file with autodetection of language.


## Running

### Ubuntu or other Debian-based Linux Distributions

1. `# apt install rustup` (as root)
2. `$ rustup default stable` (as normal user)
3. navigate to main folder (where Cargo.toml is located)
4. `$ cargo run` (as normal user)

## Known Limitations

* Will only recognize and encode the first video stream of the input file.